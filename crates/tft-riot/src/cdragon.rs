//! Downloads from Community Dragon.

use anyhow::{Context, Result, bail};
use tft_core::patches::{Calendar, LISTING_URL, ListingEntry};

/// Downloads the patch calendar. The caller saves it; on failure it keeps the saved one.
pub async fn fetch_calendar(http: &reqwest::Client) -> Result<Calendar> {
    let entries: Vec<ListingEntry> = http
        .get(LISTING_URL)
        .send()
        .await
        .context("downloading the patch list")?
        .error_for_status()?
        .json()
        .await
        .context("reading the patch list")?;
    let cal = Calendar::from_listing(&entries);
    if cal.is_empty() {
        bail!("no patches listed");
    }
    Ok(cal)
}
