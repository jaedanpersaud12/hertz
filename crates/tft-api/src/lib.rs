//! The read-only `/api` routes the React app calls, and the built app itself, served the way `src/server/index.ts`
//! serves them (`tft serve`). Every answer matches the TypeScript API exactly; `npm run parity` checks it.
//!
//! Not here yet: the summoner routes (`/api/me*`) and the routes that change your lists (POST `/api/lists/*`,
//! POST and DELETE `/api/teams*`). They answer 501 so nothing mistakes them for working. The dev-only `?data=`
//! stress switch stays in the TypeScript server.

mod describe;
pub mod js;
mod routes;

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
}

pub struct Options {
    pub db_path: PathBuf,
    pub port: u16,
}

/// Opens (or creates) the database, then serves the API and the built app on 127.0.0.1 until stopped.
pub async fn serve(opts: Options) -> Result<()> {
    // Creates the file and tables on first run, so the app opens before any crawl.
    drop(tft_store::open(&opts.db_path)?);
    let parent = opts.db_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let icons = parent.join("icons");
    std::fs::create_dir_all(&icons).with_context(|| format!("creating {}", icons.display()))?;
    tft_riot::init_tls();
    let app = Arc::new(App {
        pool: Pool { path: opts.db_path.clone(), idle: Mutex::new(Vec::new()) },
        comps: Mutex::default(),
        icons,
        dist: Path::new("dist/index.html").exists().then(|| PathBuf::from("dist")),
        // Like Node's fetch: no proxy, 15 seconds.
        http: reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(15)).build()?,
    });
    let router = axum::Router::new().fallback(handle).with_state(app);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", opts.port))
        .await
        .with_context(|| format!("port {} is in use or not allowed", opts.port))?;
    println!("Hertz running at http://127.0.0.1:{}", opts.port);
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
fn not_ported() -> Response {
    respond(
        StatusCode::NOT_IMPLEMENTED,
        "application/json",
        r#"{"error":"This route is not in the Rust engine yet. Use npm start for it."}"#,
    )
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
        ["", "api", "icon", kind, id] if !kind.is_empty() && !id.is_empty() => Route::Icon(p(kind), p(id)),
        _ => return None,
    })
}

/// The routes the TypeScript server has that this one does not yet.
fn is_unported(method: &Method, path: &str) -> bool {
    let seg: Vec<&str> = path.split('/').collect();
    let one = |s: &&str| !s.is_empty();
    match seg.as_slice() {
        ["", "api", "me"] | ["", "api", "me", "matches"] => method == Method::GET,
        ["", "api", "me", "account"] | ["", "api", "me", "refresh"] => method == Method::POST,
        ["", "api", "lists", "mark"] | ["", "api", "lists", "pin"] | ["", "api", "teams"] => method == Method::POST,
        ["", "api", "teams", id] if one(id) => method == Method::DELETE,
        _ => false,
    }
}

async fn handle(State(app): State<Arc<App>>, req: Request<Body>) -> Response {
    // Hono answers HEAD as GET (the body is dropped on the way out).
    let method = if req.method() == Method::HEAD { Method::GET } else { req.method().clone() };
    let path = js::hono_path(req.uri().path());
    let query = req.uri().query().map(str::to_string);
    if is_unported(&method, &path) {
        return not_ported();
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
        assert!(is_unported(&Method::GET, "/api/me"));
        assert!(is_unported(&Method::DELETE, "/api/teams/3"));
        assert!(!is_unported(&Method::GET, "/api/lists"));
        assert_eq!(mime_type("dist/assets/app.JS"), Some("text/javascript; charset=utf-8"));
        assert_eq!(mime_type("dist/x.unknownext"), None);
    }
}
