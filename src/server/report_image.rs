//! `GET /api/v1/reports/image` — a report chapter as a PNG.
//!
//! **Rendered here rather than in the browser**, which is the decision this
//! endpoint exists to express: a share image is a deliverable, and an Android
//! or iOS client should ask for one instead of reimplementing a layout, a type
//! scale and a palette that would then be free to drift from the web's. See
//! `crate::report_image` for the rendering itself.
//!
//! Two things it deliberately does not do. It **computes nothing**: the numbers
//! come from `reports()`, called the way the MCP tools call handlers, so there
//! is one implementation of the fold and a share cannot disagree with the
//! chapter it was shared from. And it makes **no network call**, because the
//! card draws no remote artwork — which is what keeps it free of the deadline
//! every other outbound-from-a-handler path in this codebase has had to grow.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::report_image::{self, Card, Size, Theme, Unit};
use crate::server::auth::ReadAuth;
use crate::server::models::ErrorResponse;
use crate::server::AppState;

/// The largest report page we will read back out of the handler.
const MAX_REPORT_BYTES: usize = 8 * 1024 * 1024;

fn err(code: u16, msg: &str) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImageQuery {
    /// `week`, `month` (default) or `year`. Each has its own composition —
    /// there is no card that is another card at a different size.
    pub unit: Option<String>,
    /// How many periods back from the newest, matching the Reports stepper.
    pub offset: Option<usize>,
    /// `story` (default), `square` or `wide`.
    pub size: Option<String>,
    /// `dark` (default) or `light`. **Asked at export time, not inherited** — a
    /// share lands on somebody else's feed rather than inside the app, so the
    /// reader's own theme preference is not the question being answered.
    pub theme: Option<String>,
    /// `png` (default) or `svg`. The SVG is the same document the PNG is
    /// rasterised from: smaller, sharp at any size, and something a native
    /// client can draw itself. It is *not* a second renderer — if the two ever
    /// disagreed it would be because one of them was not this one.
    pub format: Option<String>,
}

/// A report chapter as a shareable PNG, rendered on the server.
///
/// Scope: `read` *(since v0.96)*.
///
/// **Rendered on the server on purpose.** A share image is a deliverable
/// rather than a view: a native client asks for one instead of
/// reimplementing a layout, a type scale and a palette that would then be
/// free to drift from the web's. The fonts are embedded in the binary and
/// no artwork is fetched, so the same period produces the same bytes on any
/// host.
///
/// It **computes nothing**. Every figure comes from `GET /api/v1/reports`
/// with the same `unit` and `offset`, so a share cannot disagree with the
/// chapter it was shared from — including the rule that the only
/// comparison on the card is against the caller's *own* periods.
///
/// The card is a **composition, not a screenshot of the chapter**, and
/// each unit has its own — never one card relabelled, the same reason the
/// app has three chapter components:
///
/// * **week** — a headline, the **by-day bars**, the top artist and the
///   track of the week, the counts, the crush. Deliberately **no genre
///   split**: seven days make a thin one, which is the chapter's own
///   judgement.
/// * **month** — the top artist and record, the counts, the top genres,
///   what was on repeat, the crush. No by-week bars: five columns say less
///   than the ranked genre list beneath them.
/// * **year** — the **by-month bars** and the **genre bands** (the three
///   biggest genres tracked by name across the year, everything else folded
///   into `other`), the artist and record of the year, what was discovered,
///   the crush.
///
/// A section that would not fit is **left out whole** rather than clipped —
/// a PNG has no scrollbar, so a section drawn over the footer would look
/// deliberate. A thin period therefore draws fewer sections rather than
/// empty headings, and a full year gives up its discovery line before it
/// gives up the crush.
#[utoipa::path(
    get,
    path = "/api/v1/reports/image",
    tag = "Statistics",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(ImageQuery),
    responses(
        (status = 200, description = "The image. `Cache-Control` is `private`: it is one person's listening, and a shared cache must not hold it.", content(
            (crate::openapi::Bytes = "image/png"),
            (String = "image/svg+xml"),
        ), headers(("x-tapedeck-glyph-coverage" = String, description = "`full`, or `partial` when the embedded fonts could not draw every character on the card. Not an error — the host's own fonts may cover it — but a client can say \"this may not render your Korean titles\" instead of the user finding out from the picture."))),
        (status = 400, description = "An unsupported unit, size or format.", body = ErrorResponse),
        (status = 404, description = "No report for that period — an offset past the end of the history, or an instance with nothing in it yet. A real answer, not a failure.", body = ErrorResponse),
    )
)]
pub async fn report_image(
    State(state): State<Arc<AppState>>,
    auth: ReadAuth,
    Query(q): Query<ImageQuery>,
) -> impl IntoResponse {
    // A week, a month and a year are three compositions, never one card at a
    // different size — the reason Reports has three chapter components. An
    // unknown bucket is refused rather than defaulted, because the wrong one
    // would be an answer that looks entirely right.
    let unit_str = q.unit.clone().unwrap_or_else(|| "month".into());
    let Some(unit) = Unit::parse(&unit_str) else {
        return err(400, "unit must be week, month or year");
    };
    let Some(size) = Size::parse(q.size.as_deref()) else {
        return err(400, "size must be story, square or wide");
    };
    let theme = Theme::parse(q.theme.as_deref());
    let svg_only = matches!(q.format.as_deref(), Some("svg"));
    if !matches!(q.format.as_deref(), None | Some("png") | Some("svg")) {
        return err(400, "format must be png or svg");
    }
    let offset = q.offset.unwrap_or(0);

    // One period, from the handler that owns the arithmetic. `limit=1` is not
    // an optimisation — the fold walks the whole history whatever is asked for
    // — it is so the page cannot be misread for the period.
    let rq = crate::server::reports::ReportQuery {
        unit: Some(unit.as_str().to_string()),
        offset: Some(offset),
        limit: Some(1),
    };
    let resp = crate::server::reports::reports(State(state.clone()), auth, Query(rq))
        .await
        .into_response();
    let status = resp.status();
    let bytes = match axum::body::to_bytes(resp.into_body(), MAX_REPORT_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Report body for the image could not be read: {e}");
            return err(500, "Could not read your listening");
        }
    };
    if !status.is_success() {
        // The report's own message, rather than one invented here.
        return (status, bytes).into_response();
    }
    let page: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Report body for the image did not parse: {e}");
            return err(500, "Could not read your listening");
        }
    };

    let Some(period) = page.get("periods").and_then(|p| p.get(0)) else {
        // A real answer, not a failure: an offset past the end of a history, or
        // an instance with nothing in it yet.
        return err(404, "There is no report for that period yet");
    };
    let card = Card::from_period(period);

    // **Off the async worker.** A 1080×1920 pixmap is ~8 MB of RGBA and
    // rasterising it in the handler pins a tokio worker — one of four on a Pi —
    // which is the mistake the report fold itself had to be moved off.
    let rendered =
        match tokio::task::spawn_blocking(move || report_image::render(&card, unit, size, theme))
            .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::error!("Report image could not be drawn: {e}");
                return err(500, "Could not draw the image");
            }
            Err(e) => {
                tracing::error!("Report image render panicked: {e}");
                return err(500, "Could not draw the image");
            }
        };

    let stem = format!(
        "tapedeck-{}-{}-{}",
        slug(period.get("key").and_then(serde_json::Value::as_str).unwrap_or(unit.as_str())),
        size.as_str(),
        theme.as_str()
    );

    // Whether the *embedded* faces could draw everything on the card. Not an
    // error and not a reason to withhold the image: the host's own fonts sit
    // behind ours and may well cover it. It is here so a client can say "this
    // may not render your Korean titles" instead of the user finding out from
    // the picture — the discipline a source declaring its blind spots follows.
    let coverage = if rendered.fully_covered { "full" } else { "partial" };

    let (body, mime, ext) = if svg_only {
        (rendered.svg.into_bytes(), "image/svg+xml; charset=utf-8", "svg")
    } else {
        (rendered.png, "image/png", "png")
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.to_string()),
            // The image is one person's listening. `private` keeps it out of
            // any shared cache the way the artwork proxy's responses are.
            (header::CACHE_CONTROL, "private, max-age=300".to_string()),
            (header::CONTENT_DISPOSITION, format!("inline; filename=\"{stem}.{ext}\"")),
            (header::HeaderName::from_static("x-tapedeck-glyph-coverage"), coverage.to_string()),
        ],
        body,
    )
        .into_response()
}

/// A period key reduced to something safe in a filename.
///
/// The key reaches a `Content-Disposition` header, so anything that is not a
/// letter, a digit or a dash is dropped rather than escaped — a quote or a
/// newline in there is header injection, and there is nothing in a period key
/// worth keeping that this removes.
fn slug(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if out.is_empty() {
        "report".into()
    } else {
        out.chars().take(40).collect()
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(report_image))]
pub struct Api;

pub fn create_report_image_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/reports/image", get(report_image))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three buckets are drawn, and nothing else is guessed at.
    ///
    /// A unit with no composition must be **refused**, never quietly rendered
    /// as a month: the card would carry that period's numbers under the wrong
    /// heading and the wrong crush line, which is an answer that is wrong while
    /// looking entirely right.
    #[test]
    fn only_the_buckets_that_have_been_drawn_are_offered() {
        for (s, u) in [("week", Unit::Week), ("month", Unit::Month), ("year", Unit::Year)] {
            assert_eq!(Unit::parse(s), Some(u));
            assert_eq!(u.as_str(), s, "the round trip is what builds the report query");
        }
        for u in ["decade", "day", "", "MONTHS"] {
            assert_eq!(Unit::parse(u), None, "{u} has no composition");
        }
        assert_eq!(Unit::parse("  Year "), Some(Unit::Year), "trimmed and folded, like Size");
    }

    /// The period key lands in a response header, so it is reduced rather than
    /// escaped.
    #[test]
    fn a_period_key_cannot_carry_anything_into_a_header() {
        assert_eq!(slug("2026-08"), "2026-08");
        assert_eq!(slug("2026-W34"), "2026-W34");
        for bad in ["\"; x=\"", "a\r\nSet-Cookie: b", "../../etc/passwd"] {
            let out = slug(bad);
            assert!(
                out.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "{bad:?} reduced to {out:?}"
            );
        }
        assert_eq!(slug("---"), "report");
        assert!(slug(&"x".repeat(200)).len() <= 40);
    }
}
