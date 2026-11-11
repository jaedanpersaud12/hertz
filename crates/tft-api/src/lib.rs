//! The `/api` routes the React app calls, and the built app itself, served the way `src/server/index.ts` serves
//! them (`tft serve`): the stats and lists, the routes that change your lists (POST `/api/lists/*`, POST and
//! DELETE `/api/teams*`) and the summoner routes (`/api/me*`, which call Riot). Every answer, and every change to
//! the database, matches the TypeScript API exactly; `npm run golden` checks it. With `--dev` the API sits behind
//! Vite's dev server instead, and `?data=` reshapes answers into stress data (see `stress.rs`).

mod comp_admin;
mod describe;
mod engine;
pub mod js;
mod jsval;
mod me;
mod routes;
mod stress;
mod tierlist;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use regex::Regex;
use routes::{CompCache, Obj};
use sha1::{Digest, Sha1};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tft_core::config::Config;
use tft_store::Connection;

/// Read-only connections for the routes, opened as needed and reused.
struct Pool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}
impl Pool {
    fn take(&self) -> Result<Connection> {
        if let Some(c) = self.idle.lock().unwrap().pop() {
            return Ok(c);
        }
        tft_store::open_readonly(&self.path)
    }
    fn give(&self, c: Connection) {
        self.idle.lock().unwrap().push(c);
    }
}

struct App {
    pool: Pool,
    comps: CompCache,
    icons: PathBuf,
    /// The built app (`dist/`), when `dist/index.html` existed at start.
    dist: Option<PathBuf>,
    http: reqwest::Client,
    /// The one connection that writes: your lists and your own matches.
    write: Arc<Mutex<Connection>>,
    me: Arc<me::Me>,
    /// `.env` settings: the crawl defaults the settings routes fall back to.
    config: Config,
    /// `--dev`: the `?data=` stress switch is on.
    dev: bool,
}

pub struct Options {
    pub db_path: PathBuf,
    pub port: u16,
    /// The Riot key and base URL, and your Riot ID and server from .env, for the summoner routes.
    pub config: Config,
    /// `npm run dev`: no built app, and `?data=worst | empty | one | many` reshapes GET `/api/*` answers.
    pub dev: bool,
}

/// Opens (or creates) the database, then serves the API and the built app on 127.0.0.1 until stopped.
pub async fn serve(opts: Options) -> Result<()> {
    // Creates the file and tables on first run, so the app opens before any crawl.
    let write = tft_store::open(&opts.db_path)?;
    // The patch calendar is read once at start, as the TypeScript server reads it.
    let calendar = tft_store::load_calendar(&write)?;
    let write = Arc::new(Mutex::new(write));
    let parent = opts.db_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let icons = parent.join("icons");
    std::fs::create_dir_all(&icons).with_context(|| format!("creating {}", icons.display()))?;
    tft_riot::init_tls();
    let me = Arc::new(me::Me::new(Arc::clone(&write), &opts.config, calendar));
    let app = Arc::new(App {
        pool: Pool { path: opts.db_path.clone(), idle: Mutex::new(Vec::new()) },
        comps: Mutex::default(),
        icons,
        // With --dev the API sits behind Vite's dev server, which serves the app.
        dist: (!opts.dev && Path::new("dist/index.html").exists()).then(|| PathBuf::from("dist")),
        // Like Node's fetch: no proxy, 15 seconds.
        http: reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(15)).build()?,
        write,
        me,
        config: opts.config.clone(),
        dev: opts.dev,
    });
    let router = axum::Router::new().fallback(handle).with_state(app);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", opts.port))
        .await
        .with_context(|| format!("port {} is in use or not allowed", opts.port))?;
    if opts.dev {
        println!("API on http://127.0.0.1:{}", opts.port);
    } else {
        println!("Hertz running at http://127.0.0.1:{}", opts.port);
    }
    axum::serve(listener, router).await?;
    Ok(())
}

fn respond(status: StatusCode, content_type: &str, body: impl Into<Body>) -> Response {
    Response::builder().status(status).header(header::CONTENT_TYPE, content_type).body(body.into()).unwrap()
}
fn json(body: Obj) -> Response {
    respond(StatusCode::OK, "application/json", serde_json::to_vec(&body).unwrap())
}
fn not_found() -> Response {
    respond(StatusCode::NOT_FOUND, "text/plain; charset=UTF-8", "404 Not Found")
}
fn server_error(err: anyhow::Error) -> Response {
    eprintln!("{err:#}");
    respond(StatusCode::INTERNAL_SERVER_ERROR, "text/plain; charset=UTF-8", "Internal Server Error")
}
fn json_status(status: u16, body: Obj) -> Response {
    respond(StatusCode::from_u16(status).unwrap(), "application/json", serde_json::to_vec(&body).unwrap())
}

enum Route {
    Meta,
    Static,
    Comps,
    Comp(String),
    Units,
    Unit(String),
    Items,
    Item(String),
    Traits,
    Explore,
    Lists,
    Health,
    CrawlSettings,
    Icon(String, String),
}

/// Matches a decoded path the way Hono's router does: exact segments, `:param` is one non-empty segment.
fn route(path: &str) -> Option<Route> {
    let seg: Vec<&str> = path.split('/').collect();
    let p = |s: &str| js::try_decode_component(s);
    Some(match seg.as_slice() {
        ["", "api", "meta"] => Route::Meta,
        ["", "api", "static"] => Route::Static,
        ["", "api", "comps"] => Route::Comps,
        ["", "api", "comps", id] if !id.is_empty() => Route::Comp(p(id)),
        ["", "api", "units"] => Route::Units,
        ["", "api", "units", id] if !id.is_empty() => Route::Unit(p(id)),
        ["", "api", "items"] => Route::Items,
        ["", "api", "items", id] if !id.is_empty() => Route::Item(p(id)),
        ["", "api", "traits"] => Route::Traits,
        ["", "api", "explore"] => Route::Explore,
        ["", "api", "lists"] => Route::Lists,
        ["", "api", "health"] => Route::Health,
        ["", "api", "settings", "crawl"] => Route::CrawlSettings,
        ["", "api", "icon", kind, id] if !kind.is_empty() && !id.is_empty() => Route::Icon(p(kind), p(id)),
        _ => return None,
    })
}

/// The routes that change the database or call Riot.
enum Action {
    Me,
    MyMatches,
    SetAccount,
    Refresh,
    Mark,
    Pin,
    SaveTeam,
    DeleteTeam(String),
    SetCrawlSettings,
    ResetCrawlSettings,
}

fn action(method: &Method, path: &str) -> Option<Action> {
    let seg: Vec<&str> = path.split('/').collect();
    let (get, post, delete) = (method == Method::GET, method == Method::POST, method == Method::DELETE);
    Some(match seg.as_slice() {
        ["", "api", "me"] if get => Action::Me,
        ["", "api", "me", "matches"] if get => Action::MyMatches,
        ["", "api", "me", "account"] if post => Action::SetAccount,
        ["", "api", "me", "refresh"] if post => Action::Refresh,
        ["", "api", "lists", "mark"] if post => Action::Mark,
        ["", "api", "lists", "pin"] if post => Action::Pin,
        ["", "api", "teams"] if post => Action::SaveTeam,
        ["", "api", "settings", "crawl"] if post => Action::SetCrawlSettings,
        ["", "api", "settings", "crawl", "reset"] if post => Action::ResetCrawlSettings,
        ["", "api", "teams", id] if delete && !id.is_empty() => Action::DeleteTeam(js::try_decode_component(id)),
        _ => return None,
    })
}

/// A change made on the write connection, off the async threads.
type Write = Box<dyn FnOnce(&Connection) -> Result<Obj> + Send>;

/// Runs one of the [`Action`] routes. Anything the TypeScript handler would throw on is a 500.
async fn act(app: &Arc<App>, a: Action, body: Body) -> Result<Response> {
    let bytes = axum::body::to_bytes(body, usize::MAX).await?;
    let parsed = jsval::parse_body(&bytes);
    // `await c.req.json()` without a catch: a body that is not JSON throws.
    let strict = || parsed.clone().ok_or_else(|| anyhow::anyhow!("the request body is not JSON"));
    let write = |f: Write| {
        let db = Arc::clone(&app.write);
        async move { tokio::task::spawn_blocking(move || f(&db.lock().unwrap())).await? }
    };
    Ok(match a {
        Action::Me => json(app.me.get().await?),
        Action::MyMatches => {
            let me = Arc::clone(&app.me);
            json(tokio::task::spawn_blocking(move || me.matches()).await??)
        }
        Action::SetAccount => {
            let (status, body) = app.me.set_account(parsed).await?;
            json_status(status, body)
        }
        Action::Refresh => json(app.me.refresh_now().await?),
        Action::Mark => {
            let b = strict()?;
            json(write(Box::new(move |c| routes::mark(c, &b))).await?)
        }
        Action::Pin => {
            let b = strict()?;
            json(write(Box::new(move |c| routes::pin(c, &b))).await?)
        }
        Action::SaveTeam => {
            let b = strict()?;
            json(write(Box::new(move |c| routes::save_team(c, &b))).await?)
        }
        Action::DeleteTeam(id) => json(write(Box::new(move |c| routes::delete_team(c, &id))).await?),
        Action::SetCrawlSettings => {
            let (db, config) = (Arc::clone(&app.write), app.config.clone());
            let (status, body) =
                tokio::task::spawn_blocking(move || engine::set_settings(&db.lock().unwrap(), &config, &bytes)).await??;
            json_status(status, body)
        }
        Action::ResetCrawlSettings => {
            let config = app.config.clone();
            json(write(Box::new(move |c| engine::reset_settings(c, &config))).await?)
        }
    })
}

async fn handle(State(app): State<Arc<App>>, req: Request<Body>) -> Response {
    // Dev only: ?data=worst (and friends) reshapes a GET /api/* JSON answer into stress data. See stress.rs.
    let mode = (app.dev && req.method() == Method::GET)
        .then(|| js::query(req.uri().query(), "data"))
        .flatten()
        .filter(|m| !m.is_empty());
    let path = js::hono_path(req.uri().path());
    let res = answer(app, req).await;
    let Some(mode) = mode else { return res };
    let is_json = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("json"));
    if !(path == "/api" || path.starts_with("/api/")) || !is_json {
        return res;
    }
    let Ok(bytes) = axum::body::to_bytes(res.into_body(), usize::MAX).await else {
        return server_error(anyhow::anyhow!("reading an answer to reshape"));
    };
    let Ok(mut body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return server_error(anyhow::anyhow!("an answer to reshape is not JSON"));
    };
    stress::stress(&mode, &path, &mut body);
    // Like `c.json(body)`: a fresh 200 answer.
    respond(StatusCode::OK, "application/json", serde_json::to_vec(&body).unwrap())
}

async fn answer(app: Arc<App>, req: Request<Body>) -> Response {
    // Hono answers HEAD as GET (the body is dropped on the way out).
    let method = if req.method() == Method::HEAD { Method::GET } else { req.method().clone() };
    let path = js::hono_path(req.uri().path());
    let query = req.uri().query().map(str::to_string);
    if let Some(r) = comp_admin::route(&method, &path) {
        return comp_admin::handle(&app, r, req.into_body(), query).await; // PRD 2: renames, merges, variants
    }
    if let Some(a) = action(&method, &path) {
        return act(&app, a, req.into_body()).await.unwrap_or_else(server_error);
    }
    if method == Method::GET {
        if let Some(r) = route(&path) {
            if let Route::Icon(kind, id) = r {
                return icon(&app, &kind, &id).await.unwrap_or_else(server_error);
            }
            let app2 = app.clone();
            let result = tokio::task::spawn_blocking(move || -> Result<Obj> {
                let conn = app2.pool.take()?;
                let q = query.as_deref();
                let out = match r {
                    Route::Meta => routes::meta(&conn, q),
                    Route::Static => routes::static_data(&conn),
                    Route::Comps => routes::comps(&conn, q, &app2.comps),
                    Route::Comp(id) => routes::comp(&conn, q, &id, &app2.comps),
                    Route::Units => routes::units(&conn, q),
                    Route::Unit(id) => routes::unit(&conn, q, &id, &app2.comps),
                    Route::Items => routes::items(&conn, q),
                    Route::Item(id) => routes::item(&conn, q, &id),
                    Route::Traits => routes::traits(&conn, q),
                    Route::Explore => routes::explore(&conn, q),
                    Route::Lists => routes::lists(&conn),
                    Route::Health => engine::health(&conn, &app2.config),
                    Route::CrawlSettings => engine::get_settings(&conn, &app2.config),
                    Route::Icon(..) => unreachable!(),
                };
                app2.pool.give(conn);
                out
            })
            .await;
            return match result {
                Ok(Ok(body)) => json(body),
                Ok(Err(e)) => server_error(e),
                Err(e) => server_error(e.into()),
            };
        }
    }
    let Some(dist) = &app.dist else { return not_found() };
    if let Some(r) = static_file(dist, &path, req.method()) {
        return r;
    }
    if method == Method::GET {
        // The app's own routing takes every other path.
        return match std::fs::read(dist.join("index.html")) {
            Ok(b) => respond(StatusCode::OK, "text/html; charset=UTF-8", String::from_utf8_lossy(&b).into_owned()),
            Err(e) => server_error(e.into()),
        };
    }
    not_found()
}

/// Icons are downloaded from Community Dragon once, then served from disk.
async fn icon(app: &Arc<App>, kind: &str, id: &str) -> Result<Response> {
    let (kind, id, app2) = (kind.to_string(), id.to_string(), app.clone());
    let source = tokio::task::spawn_blocking(move || -> Result<Option<String>> {
        let conn = app2.pool.take()?;
        let out = routes::icon_source(&conn, &kind, &id);
        app2.pool.give(conn);
        out
    })
    .await??;
    let Some(source) = source else { return Ok(not_found()) };
    let hash: String = Sha1::digest(source.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let file = app.icons.join(format!("{hash}.png"));
    if !file.exists() {
        let Ok(res) = app.http.get(&source).send().await else { return Ok(not_found()) };
        if !res.status().is_success() {
            return Ok(not_found());
        }
        let Ok(bytes) = res.bytes().await else { return Ok(not_found()) };
        std::fs::write(&file, &bytes)?;
    }
    let body = std::fs::read(&file)?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(Body::from(body))
        .unwrap())
}

/// `serveStatic({ root: './dist' })` from `@hono/node-server`: a file under dist, or a folder's index.html.
fn static_file(dist: &Path, path: &str, method: &Method) -> Option<Response> {
    static ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[/\\])\.{1,2}(?:$|[/\\])|[/\\]{2,}|\\").unwrap());
    let name = js::try_decode_uri(path);
    if ESCAPE.is_match(&name) {
        return None;
    }
    // Like path.join: the request path is relative to dist, and "." segments drop out.
    let mut file = dist.to_path_buf();
    for c in Path::new(name.trim_start_matches('/')).components() {
        match c {
            Component::Normal(p) => file.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    let mut meta = std::fs::metadata(&file).ok();
    if meta.as_ref().is_some_and(|m| m.is_dir()) {
        file.push("index.html");
        meta = std::fs::metadata(&file).ok();
    }
    let meta = meta?;
    let mime = mime_type(&file.to_string_lossy()).unwrap_or("application/octet-stream");
    let body = if method == Method::HEAD || method == Method::OPTIONS { Vec::new() } else { std::fs::read(&file).ok()? };
    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CONTENT_LENGTH, meta.len())
            .body(Body::from(body))
            .unwrap(),
    )
}

/// Hono's `getMimeType`: the extension after the last dot, letters and digits only.
fn mime_type(path: &str) -> Option<&'static str> {
    static EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.([a-zA-Z0-9]+?)$").unwrap());
    let ext = EXT.captures(path)?.get(1)?.as_str().to_ascii_lowercase();
    MIMES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

const MIMES: &[(&str, &str)] = &[
    ("aac", "audio/aac"), ("avi", "video/x-msvideo"), ("avif", "image/avif"), ("av1", "video/av1"),
    ("bin", "application/octet-stream"), ("bmp", "image/bmp"), ("css", "text/css; charset=utf-8"),
    ("csv", "text/csv; charset=utf-8"), ("eot", "application/vnd.ms-fontobject"), ("epub", "application/epub+zip"),
    ("gif", "image/gif"), ("gz", "application/gzip"), ("htm", "text/html; charset=utf-8"),
    ("html", "text/html; charset=utf-8"), ("ico", "image/x-icon"), ("ics", "text/calendar; charset=utf-8"),
    ("jpeg", "image/jpeg"), ("jpg", "image/jpeg"), ("js", "text/javascript; charset=utf-8"),
    ("json", "application/json"), ("jsonld", "application/ld+json"), ("map", "application/json"),
    ("mid", "audio/x-midi"), ("midi", "audio/x-midi"), ("mjs", "text/javascript; charset=utf-8"),
    ("mp3", "audio/mpeg"), ("mp4", "video/mp4"), ("mpeg", "video/mpeg"), ("oga", "audio/ogg"), ("ogv", "video/ogg"),
    ("ogx", "application/ogg"), ("opus", "audio/opus"), ("otf", "font/otf"), ("pdf", "application/pdf"),
    ("png", "image/png"), ("rtf", "application/rtf"), ("svg", "image/svg+xml; charset=utf-8"), ("tif", "image/tiff"),
    ("tiff", "image/tiff"), ("ts", "video/mp2t"), ("ttf", "font/ttf"), ("txt", "text/plain; charset=utf-8"),
    ("wasm", "application/wasm"), ("webm", "video/webm"), ("weba", "audio/webm"),
    ("webmanifest", "application/manifest+json"), ("webp", "image/webp"), ("woff", "font/woff"),
    ("woff2", "font/woff2"), ("xhtml", "application/xhtml+xml; charset=utf-8"),
    ("xml", "application/xml; charset=utf-8"), ("zip", "application/zip"), ("3gp", "video/3gpp"),
    ("3g2", "video/3gpp2"), ("gltf", "model/gltf+json"), ("glb", "model/gltf-binary"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_like_hono() {
        assert!(matches!(route("/api/comps"), Some(Route::Comps)));
        assert!(matches!(route("/api/comps/A|B"), Some(Route::Comp(id)) if id == "A|B"));
        assert!(matches!(route("/api/units/a%2Fb"), Some(Route::Unit(id)) if id == "a/b"));
        assert!(route("/api/comps/").is_none());
        assert!(route("/api/comps/a/b").is_none());
        assert!(route("/api/icon/unit/").is_none());
        assert!(matches!(action(&Method::GET, "/api/me"), Some(Action::Me)));
        assert!(matches!(action(&Method::DELETE, "/api/teams/3"), Some(Action::DeleteTeam(id)) if id == "3"));
        assert!(matches!(action(&Method::DELETE, "/api/teams/%201"), Some(Action::DeleteTeam(id)) if id == " 1"));
        assert!(action(&Method::DELETE, "/api/teams/").is_none());
        assert!(action(&Method::GET, "/api/teams/3").is_none());
        assert!(action(&Method::POST, "/api/me").is_none());
        assert!(action(&Method::GET, "/api/lists").is_none());
        assert_eq!(mime_type("dist/assets/app.JS"), Some("text/javascript; charset=utf-8"));
        assert_eq!(mime_type("dist/x.unknownext"), None);
    }
}
