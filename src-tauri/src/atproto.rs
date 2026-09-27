//! AT Protocol OAuth: identity resolution, discovery, and the token dance.
//!
//! Deliberately its own module rather than a mode of [`crate::oauth`]. The two
//! share the words "OAuth" and "`PKCE`" and almost nothing else — this one adds
//! mandatory `PAR`, mandatory `DPoP`, a client identity published as a document on
//! the web, and a discovery chain that starts from the user's handle. Bending
//! the loopback flow to cover both would make each harder to read.
//!
//! ## The chain, in order
//!
//! ``text
//! handle  →  DID            (well-known, then a public resolver)
//! DID     →  DID document   (plc.directory, or the domain for did:web)
//! doc     →  handle check   (MANDATORY: the document must claim the handle back)
//! doc     →  `PDS`            (#atproto_pds service entry)
//! `PDS`     →  auth server    (/.well-known/oauth-protected-resource)
//! server  →  endpoints      (/.well-known/oauth-authorization-server)
//! ``
//!
//! The bidirectional handle check is not optional politeness: without it,
//! anyone who can set a DNS record could point a handle at a DID they do not
//! control, and the app would happily connect to it.
//!
//! ## The client identity is a URL
//!
//! There is no registration step and no client secret. The `client_id` IS a
//! public HTTPS URL serving a metadata document, and the authorization server
//! fetches it during `PAR`. That means **the metadata must be live on the web
//! before this flow can work at all** — see [`preflight_client_metadata`], which
//! turns that into a checklist rather than an opaque `PAR` rejection.
//!
//! The redirect must be a custom URI scheme matching the `client_id` hostname in
//! reverse-domain order; a loopback redirect — what every other platform here
//! uses — is invalid for a production native client.

use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::dpop;
use crate::error::{AppError, Result, from_status};
use crate::http;

/// Where Windbag's own client metadata is published. Overridable per install
/// (Settings → Platform apps) so a fork, or anyone self-hosting, can point at
/// their own document without rebuilding.
pub const DEFAULT_CLIENT_ID: &str = "https://entro314-labs.github.io/yapper/client-metadata.json";

/// `atproto` is mandatory for every client. `transition:generic` is what grants
/// the record writes this app exists to make — verified against
/// `bsky.social`'s advertised `scopes_supported` on 2026-08-28.
pub const SCOPES: &str = "atproto transition:generic";

/// The URI scheme the OS routes back to this app. MUST equal
/// [`redirect_scheme`] of [`DEFAULT_CLIENT_ID`] and the `deep-link` scheme in
/// `tauri.conf.json`; a test below pins the first of those and the second is
/// what makes the callback arrive at all.
pub const CALLBACK_SCHEME: &str = "io.github.entro314-labs";

/// How long the browser handoff may take before it is abandoned.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

// ─── Identity ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Identity {
    pub did: String,
    pub handle: String,
    pub pds: String,
}

/// Resolves a handle (or a bare DID) all the way to its `PDS`.
pub fn resolve_identity(input: &str) -> Result<Identity> {
    let input = input.trim().trim_start_matches('@');
    if input.is_empty() {
        return Err(AppError::InvalidInput("Enter your handle.".into()));
    }

    let (did, handle) = if input.starts_with("did:") {
        let document = fetch_did_document(input)?;
        let handle = document
            .handle()
            .ok_or_else(|| AppError::InvalidInput(format!("{input} claims no handle.")))?;
        (input.to_string(), handle)
    } else {
        let did = resolve_handle(input)?;
        let document = fetch_did_document(&did)?;
        // MANDATORY per the spec: the document must claim the handle back.
        // Without this, a DNS record alone could point a handle at someone
        // else's DID and the app would connect to the wrong account.
        if !document.claims_handle(input) {
            return Err(AppError::InvalidInput(format!(
                "`{input}` resolves to {did}, but that account does not claim the handle back. \
                 Refusing to connect."
            )));
        }
        (did, input.to_string())
    };

    let document = fetch_did_document(&did)?;
    let pds = document.pds().ok_or_else(|| {
        AppError::InvalidInput(format!("{did} has no PDS in its identity document."))
    })?;
    Ok(Identity { did, handle, pds })
}

/// Handle → DID.
///
/// The decentralised route first: a handle's own domain may serve the answer at
/// `/.well-known/atproto-did`. Most handles do not — `*.bsky.social` handles are
/// resolved by DNS, which this has no resolver for — so a public, unauthenticated
/// resolver is the fallback. It sees the handle and nothing else.
fn resolve_handle(handle: &str) -> Result<String> {
    if let Ok(response) = http::client()
        .get(format!("https://{handle}/.well-known/atproto-did"))
        .send()
    {
        let (status, body) = http::read_body(response);
        let candidate = body.trim();
        if (200..300).contains(&status) && candidate.starts_with("did:") {
            return Ok(candidate.to_string());
        }
    }

    let (status, body) = http::read_body(
        http::client()
            .get("https://public.api.bsky.app/xrpc/com.atproto.identity.resolveHandle")
            .query(&[("handle", handle)])
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(AppError::InvalidInput(format!(
            "`{handle}` is not a handle Bluesky can resolve."
        )));
    }
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("did")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| AppError::Platform("The handle resolver returned no DID.".into()))
}

#[derive(Debug, Deserialize)]
struct DidDocument {
    #[serde(default, rename = "alsoKnownAs")]
    also_known_as: Vec<String>,
    #[serde(default)]
    service: Vec<DidService>,
}

#[derive(Debug, Deserialize)]
struct DidService {
    id: String,
    #[serde(rename = "serviceEndpoint")]
    endpoint: String,
}

impl DidDocument {
    fn claims_handle(&self, handle: &str) -> bool {
        let expected = format!("at://{}", handle.to_ascii_lowercase());
        self.also_known_as
            .iter()
            .any(|value| value.to_ascii_lowercase() == expected)
    }

    fn handle(&self) -> Option<String> {
        self.also_known_as
            .iter()
            .find_map(|value| value.strip_prefix("at://").map(str::to_owned))
    }

    /// The `#atproto_pds` service endpoint. Matched on the id's suffix because
    /// a document may write it fully-qualified (`did:plc:…#atproto_pds`).
    fn pds(&self) -> Option<String> {
        self.service
            .iter()
            .find(|service| service.id.ends_with("#atproto_pds"))
            .map(|service| service.endpoint.trim_end_matches('/').to_string())
    }
}

fn fetch_did_document(did: &str) -> Result<DidDocument> {
    let url = if let Some(domain) = did.strip_prefix("did:web:") {
        // `did:web:example.com` serves its own document.
        format!("https://{domain}/.well-known/did.json")
    } else if did.starts_with("did:plc:") {
        format!("https://plc.directory/{did}")
    } else {
        return Err(AppError::InvalidInput(format!(
            "`{did}` is not a DID method Windbag can resolve (did:plc and did:web only)."
        )));
    };

    let (status, body) = http::read_body(http::client().get(&url).send()?);
    if !(200..300).contains(&status) {
        return Err(AppError::InvalidInput(format!(
            "Could not read the identity document for {did} ({status})."
        )));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("{did} has an unreadable identity document: {e}")))
}

// ─── Server discovery ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct AuthServer {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// `PAR` is mandatory in this profile, so a server without it cannot be used.
    #[serde(default)]
    pub pushed_authorization_request_endpoint: Option<String>,
}

/// `PDS` → its authorization server → that server's endpoints.
pub fn discover_auth_server(pds: &str) -> Result<AuthServer> {
    #[derive(Deserialize)]
    struct ProtectedResource {
        #[serde(default)]
        authorization_servers: Vec<String>,
    }

    let (status, body) = http::read_body(
        http::client()
            .get(format!("{pds}/.well-known/oauth-protected-resource"))
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(AppError::Platform(format!(
            "{pds} does not advertise an authorization server ({status}). It may not support \
             OAuth yet — connect with an app password instead."
        )));
    }
    let resource: ProtectedResource = serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("{pds} returned unreadable metadata: {e}")))?;
    let issuer = resource
        .authorization_servers
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Platform(format!("{pds} lists no authorization server.")))?;

    let issuer = issuer.trim_end_matches('/').to_string();
    let (status, body) = http::read_body(
        http::client()
            .get(format!("{issuer}/.well-known/oauth-authorization-server"))
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Bluesky"));
    }
    let server: AuthServer = serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("{issuer} returned unreadable metadata: {e}")))?;

    // The issuer must be the server that served the document, or a token from
    // it could be minted by somebody else entirely.
    if server.issuer.trim_end_matches('/') != issuer {
        return Err(AppError::Platform(format!(
            "{issuer} identifies itself as {} — refusing to continue.",
            server.issuer
        )));
    }
    if server.pushed_authorization_request_endpoint.is_none() {
        return Err(AppError::Platform(format!(
            "{issuer} does not support pushed authorization requests, which this flow requires."
        )));
    }
    Ok(server)
}

// ─── Client identity ────────────────────────────────────────────────────────

/// The redirect URI for a given `client_id`: its hostname in reverse-domain
/// order, used as a URI scheme.
///
/// `https://entro314-labs.github.io/yapper/client-metadata.json`
///   → `io.github.entro314-labs:/callback`
///
/// Derived rather than hardcoded so overriding the client id cannot silently
/// leave a redirect the authorization server will reject.
pub fn redirect_uri(client_id: &str) -> Result<String> {
    Ok(format!("{}:/callback", redirect_scheme(client_id)?))
}

pub fn redirect_scheme(client_id: &str) -> Result<String> {
    let host = url::Url::parse(client_id)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .ok_or_else(|| {
            AppError::InvalidInput(format!("`{client_id}` is not a usable client id URL."))
        })?;
    Ok(host.split('.').rev().collect::<Vec<_>>().join("."))
}

/// Checks the client metadata is actually published before starting a flow.
///
/// Without this the first failure is a `PAR` rejection reading
/// `invalid_client_metadata`, which says nothing about the real cause — that the
/// document is not on the web yet. Cheap, and it turns a dead end into a task.
pub fn preflight_client_metadata(client_id: &str) -> Result<()> {
    let (status, body) = http::read_body(http::client().get(client_id).send().map_err(|e| {
        AppError::InvalidInput(format!(
            "Windbag's client metadata is not reachable at {client_id} ({e}). \
             Bluesky's OAuth needs that file published on the web before sign-in can start; \
             connect with an app password instead, or host the file and set its URL in \
             Settings → Platform apps."
        ))
    })?);
    if !(200..300).contains(&status) {
        return Err(AppError::InvalidInput(format!(
            "Windbag's client metadata at {client_id} answered {status}. It must be publicly \
             readable before Bluesky's OAuth can be used."
        )));
    }

    let document: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
        AppError::InvalidInput(format!("{client_id} did not serve a JSON document."))
    })?;
    // The spec requires the document's own `client_id` to equal the URL it was
    // fetched from; a mismatch fails at `PAR` with a message that names neither.
    let declared = document
        .get("client_id")
        .and_then(serde_json::Value::as_str);
    if declared != Some(client_id) {
        return Err(AppError::InvalidInput(format!(
            "The metadata at {client_id} declares client_id {}, which must match the URL it is \
             served from.",
            declared.unwrap_or("nothing")
        )));
    }
    Ok(())
}

// ─── The flow ───────────────────────────────────────────────────────────────

/// What a completed flow yields.
pub struct Session {
    pub did: String,
    pub access_token: String,
    pub refresh_token: String,
    /// RFC 3339.
    pub expires_at: Option<String>,
    pub scopes: Option<String>,
    /// The key the tokens are bound to, base64. Must be stored: a refresh signed
    /// by any other key is refused.
    pub dpop_key: String,
    pub issuer: String,
    pub pds: String,
}

/// Runs the whole browser handoff. Blocks; call it from a worker thread.
pub fn authorize(client_id: &str, handle_or_did: &str) -> Result<Session> {
    preflight_client_metadata(client_id)?;

    let identity = resolve_identity(handle_or_did)?;
    let server = discover_auth_server(&identity.pds)?;
    let redirect = redirect_uri(client_id)?;
    let key = dpop::Key::generate();

    let verifier = B64.encode(rand::random::<[u8; 32]>());
    let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
    let state = B64.encode(rand::random::<[u8; 16]>());

    // `PAR`: the request is pushed to the server first and the browser is then
    // sent to a short `request_uri`. Mandatory in this profile — parameters
    // never travel through the user agent.
    let par_endpoint = server
        .pushed_authorization_request_endpoint
        .clone()
        .unwrap_or_default();
    let request_uri = push_authorization_request(
        &key,
        &par_endpoint,
        client_id,
        &redirect,
        &state,
        &challenge,
        &identity.handle,
    )?;

    let mut authorize_url = url::Url::parse(&server.authorization_endpoint)
        .map_err(|e| AppError::Platform(format!("Malformed authorization endpoint: {e}")))?;
    authorize_url
        .query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("request_uri", &request_uri);

    let listener = CallbackListener::arm(&state);
    opener::open_browser(authorize_url.as_str())
        .map_err(|e| AppError::Internal(format!("Could not open the browser: {e}")))?;

    let callback = listener.wait(CALLBACK_TIMEOUT)?;
    let code = read_callback(&callback, &state, &server.issuer)?;

    let tokens = exchange(
        &key,
        &server.token_endpoint,
        client_id,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
        ],
        false,
    )?;

    // The account the tokens are for must be the account that was asked for.
    if tokens.sub != identity.did {
        return Err(AppError::Unauthorized(format!(
            "Signed in as {} but {} was requested. Nothing was connected.",
            tokens.sub, identity.did
        )));
    }

    let expires_at = tokens.expires_at();
    let refresh_token = tokens.refresh_token.ok_or_else(|| {
        AppError::Unauthorized(
            "Bluesky issued no refresh token, so the connection would die within the hour.".into(),
        )
    })?;

    Ok(Session {
        did: identity.did,
        access_token: tokens.access_token,
        refresh_token,
        expires_at,
        scopes: tokens.scope,
        dpop_key: key.to_base64(),
        issuer: server.issuer,
        pds: identity.pds,
    })
}

/// Trades a refresh token for a fresh access token, signed by the SAME key the
/// tokens were bound to.
pub fn refresh(
    key: &dpop::Key,
    token_endpoint: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse> {
    exchange(
        key,
        token_endpoint,
        client_id,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
        true,
    )
}

fn push_authorization_request(
    key: &dpop::Key,
    endpoint: &str,
    client_id: &str,
    redirect: &str,
    state: &str,
    challenge: &str,
    login_hint: &str,
) -> Result<String> {
    let form = [
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        ("scope", SCOPES),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        // Pre-fills the account so the user is not asked who they are twice.
        ("login_hint", login_hint),
    ];

    let (status, body) = dpop::send(key, "POST", endpoint, None, || {
        http::client().post(endpoint).form(&form)
    })?;
    if !(200..300).contains(&status) {
        return Err(par_error(status, &body, client_id));
    }

    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("request_uri")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| AppError::Platform("Bluesky's PAR response carried no request_uri.".into()))
}

/// `PAR` rejections are nearly always the client document, and the raw error names
/// neither the document nor what is wrong with it.
fn par_error(status: u16, body: &str, client_id: &str) -> AppError {
    if body.contains("invalid_client_metadata") || body.contains("invalid_client") {
        return AppError::InvalidInput(format!(
            "Bluesky rejected Windbag's client metadata at {client_id}. It must be publicly \
             readable, declare its own URL as `client_id`, and list \
             `{}` among its redirect_uris. Details: {}",
            redirect_uri(client_id).unwrap_or_default(),
            body.chars().take(200).collect::<String>()
        ));
    }
    from_status(status, body, "Bluesky")
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    /// The account DID the tokens belong to.
    #[serde(default)]
    pub sub: String,
}

impl TokenResponse {
    pub fn expires_at(&self) -> Option<String> {
        self.expires_in.map(|seconds| {
            (chrono::Utc::now() + chrono::Duration::seconds(seconds.max(60))).to_rfc3339()
        })
    }
}

fn exchange(
    key: &dpop::Key,
    endpoint: &str,
    client_id: &str,
    form: &[(&str, &str)],
    refreshing: bool,
) -> Result<TokenResponse> {
    let mut fields: Vec<(&str, &str)> = form.to_vec();
    fields.push(("client_id", client_id));

    let (status, body) = dpop::send(key, "POST", endpoint, None, || {
        http::client().post(endpoint).form(&fields)
    })?;
    if !(200..300).contains(&status) {
        if refreshing {
            return Err(crate::oauth::refresh_rejection(status, &body, "Bluesky"));
        }
        return Err(match from_status(status, &body, "Bluesky") {
            AppError::Platform(message) | AppError::Network(message) => {
                AppError::Unauthorized(message)
            }
            other => other,
        });
    }
    serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "Bluesky returned an unreadable token response: {e}"
        ))
    })
}

// ─── The callback ───────────────────────────────────────────────────────────

/// The flow a deep link can be delivered to: where to send it, and the state
/// it must carry to belong there.
struct Pending {
    sender: mpsc::Sender<String>,
    state: String,
}

/// Where a deep link lands while a flow is waiting for one.
///
/// A single slot rather than a queue: only one sign-in runs at a time (the
/// connect command already enforces that), and a stray link arriving with no
/// flow in progress should be dropped rather than queued for the next one.
fn pending() -> &'static Mutex<Option<Pending>> {
    static PENDING: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

/// Hands a callback URL to a waiting flow. Called by the deep-link handler and
/// by the manual paste fallback alike. Returns whether it was taken.
///
/// Only a URL carrying the waiting flow's own `state` is delivered. Anything
/// else — a stale link from an abandoned attempt, a crafted `?error=` — is not
/// part of this sign-in, so it is dropped here and the flow keeps waiting
/// rather than being aborted by it.
pub fn deliver_callback(url: &str) -> bool {
    let Ok(slot) = pending().lock() else {
        return false;
    };
    let Some(pending) = slot.as_ref() else {
        return false;
    };
    let belongs = callback_query(url)
        .ok()
        .and_then(|query| query_param(&query, "state"))
        .is_some_and(|state| state == pending.state);
    belongs && pending.sender.send(url.to_string()).is_ok()
}

struct CallbackListener {
    receiver: mpsc::Receiver<String>,
}

impl CallbackListener {
    /// Armed BEFORE the browser opens, so a callback that arrives immediately
    /// cannot be missed.
    fn arm(state: &str) -> Self {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut slot) = pending().lock() {
            *slot = Some(Pending {
                sender,
                state: state.to_string(),
            });
        }
        Self { receiver }
    }

    fn wait(&self, timeout: Duration) -> Result<String> {
        self.receiver.recv_timeout(timeout).map_err(|_| {
            AppError::Unauthorized(format!(
                "Timed out waiting for Bluesky to send you back ({} minutes). If your browser \
                 showed a link it could not open, paste it into the connect dialog.",
                timeout.as_secs() / 60
            ))
        })
    }
}

impl Drop for CallbackListener {
    fn drop(&mut self) {
        // Disarm, so a late callback from an abandoned flow is dropped rather
        // than delivered into whatever runs next.
        if let Ok(mut slot) = pending().lock() {
            *slot = None;
        }
    }
}

/// Reads the authorization code out of a callback URL, checking everything that
/// makes it trustworthy: the state we generated, and the issuer that answered.
pub fn read_callback(url: &str, expected_state: &str, expected_issuer: &str) -> Result<String> {
    let parsed = callback_query(url)?;
    let param = |name: &str| query_param(&parsed, name);

    if let Some(error) = param("error") {
        let description = param("error_description").unwrap_or_default();
        return Err(AppError::Unauthorized(match error.as_str() {
            "access_denied" => "Sign-in was cancelled on the Bluesky consent screen.".into(),
            other => format!("Bluesky returned an error: {other} {description}"),
        }));
    }
    if param("state").as_deref() != Some(expected_state) {
        return Err(AppError::Unauthorized(
            "The sign-in callback did not match the request Windbag started. Nothing was \
             connected; try again."
                .into(),
        ));
    }
    // The `iss` check is specific to this profile and is what stops a callback
    // from one server being replayed against another.
    match param("iss") {
        Some(issuer) if issuer.trim_end_matches('/') == expected_issuer.trim_end_matches('/') => {}
        Some(issuer) => {
            return Err(AppError::Unauthorized(format!(
                "The callback came from {issuer}, not {expected_issuer}. Nothing was connected."
            )));
        }
        None => {
            return Err(AppError::Unauthorized(
                "The callback named no issuer, which this flow requires.".into(),
            ));
        }
    }
    param("code").ok_or_else(|| {
        AppError::Unauthorized("The callback arrived without an authorization code.".into())
    })
}

/// A custom-scheme URL (`io.github.x:/callback?...`) is not a hierarchical
/// URL, so it is normalised to one before parsing — only the query matters.
fn callback_query(url: &str) -> Result<url::Url> {
    let query = url.split_once('?').map_or("", |(_, query)| query);
    url::Url::parse(&format!("https://callback.invalid/?{query}"))
        .map_err(|e| AppError::InvalidInput(format!("That is not a callback URL: {e}")))
}

fn query_param(parsed: &url::Url, name: &str) -> Option<String> {
    parsed
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redirect_scheme_is_the_client_host_reversed() {
        assert_eq!(
            redirect_uri("https://entro314-labs.github.io/yapper/client-metadata.json").unwrap(),
            "io.github.entro314-labs:/callback"
        );
        // The spec's own example.
        assert_eq!(
            redirect_uri("https://app.example.com/oauth-client-metadata.json").unwrap(),
            "com.example.app:/callback"
        );
    }

    #[test]
    fn the_registered_scheme_matches_the_default_client_id() {
        // If these drift, the browser opens a URL the OS routes nowhere and the
        // sign-in hangs until it times out with nothing to explain it.
        assert_eq!(redirect_scheme(DEFAULT_CLIENT_ID).unwrap(), CALLBACK_SCHEME);

        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let schemes = config
            .pointer("/plugins/deep-link/desktop/schemes")
            .and_then(serde_json::Value::as_array)
            .expect("the deep-link scheme must be registered");
        assert!(
            schemes.iter().any(|value| value == CALLBACK_SCHEME),
            "tauri.conf.json registers {schemes:?}, not {CALLBACK_SCHEME}"
        );
    }

    #[test]
    fn the_shipped_client_metadata_matches_what_the_flow_will_send() {
        // The published document is what the authorization server fetches. A
        // redirect_uri it does not list is refused at `PAR`.
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../../docs/client-metadata.json"))
                .expect("docs/client-metadata.json");

        assert_eq!(document["client_id"], DEFAULT_CLIENT_ID);
        assert_eq!(document["application_type"], "native");
        assert_eq!(document["dpop_bound_access_tokens"], true);
        assert_eq!(document["token_endpoint_auth_method"], "none");
        assert_eq!(document["scope"], SCOPES);

        let redirects = document["redirect_uris"].as_array().expect("redirect_uris");
        let expected = redirect_uri(DEFAULT_CLIENT_ID).unwrap();
        assert!(
            redirects.iter().any(|value| value == &expected),
            "the document lists {redirects:?}, but the flow sends {expected}"
        );

        for grant in ["authorization_code", "refresh_token"] {
            assert!(
                document["grant_types"]
                    .as_array()
                    .expect("grant_types")
                    .iter()
                    .any(|value| value == grant),
                "the document must declare the {grant} grant"
            );
        }
    }

    #[test]
    fn a_client_id_that_is_not_a_url_is_rejected_rather_than_guessed_at() {
        assert!(redirect_uri("not a url").is_err());
        assert!(redirect_uri("").is_err());
    }

    #[test]
    fn an_unreachable_client_document_names_the_url_and_the_way_out() {
        // `.invalid` is reserved and never resolves (RFC 2606), so this exercises
        // the preflight's unreachable branch without depending on the network.
        let err = preflight_client_metadata("https://windbag.invalid/client-metadata.json")
            .expect_err("unreachable");
        let message = err.to_string();
        assert!(message.contains("windbag.invalid"), "{message}");
        assert!(
            message.contains("app password"),
            "the user needs the way forward, not just the failure: {message}"
        );
        assert!(
            !err.is_retryable(),
            "an unpublished document does not appear on its own"
        );
    }

    #[test]
    fn an_unpublished_client_document_is_explained_rather_than_echoed() {
        // Bluesky's verbatim answer, captured from https://bsky.social/oauth/par
        // on 2026-08-28 with the metadata URL not yet live. The raw text names
        // neither what a client document is nor how to publish one.
        let body = r#"{"error":"invalid_client_metadata","error_description":"Unable to obtain client metadata for \"https://entro314-labs.github.io/yapper/client-metadata.json\": Not Found"}"#;
        let err = par_error(400, body, DEFAULT_CLIENT_ID);

        assert!(
            !err.is_retryable(),
            "a missing document does not appear on its own"
        );
        let message = err.to_string();
        assert!(message.contains("publicly readable"), "{message}");
        assert!(
            message.contains("io.github.entro314-labs:/callback"),
            "the message must name the redirect the document has to list: {message}"
        );
    }

    #[test]
    fn a_did_document_must_claim_the_handle_back() {
        let document = DidDocument {
            also_known_as: vec!["at://me.bsky.social".into()],
            service: Vec::new(),
        };
        assert!(document.claims_handle("me.bsky.social"));
        assert!(
            document.claims_handle("ME.BSKY.SOCIAL"),
            "handles are case-insensitive"
        );
        assert!(
            !document.claims_handle("someone-else.bsky.social"),
            "a DNS record alone must not be able to point a handle at another DID"
        );
    }

    #[test]
    fn the_pds_is_read_from_the_atproto_service_entry() {
        let document = DidDocument {
            also_known_as: vec!["at://me.test".into()],
            service: vec![
                DidService {
                    id: "#atproto_labeler".into(),
                    endpoint: "https://labeler.test".into(),
                },
                DidService {
                    id: "did:plc:abc#atproto_pds".into(),
                    endpoint: "https://pds.test/".into(),
                },
            ],
        };
        assert_eq!(document.pds().as_deref(), Some("https://pds.test"));
    }

    #[test]
    fn a_document_with_no_pds_yields_none() {
        let document = DidDocument {
            also_known_as: vec!["at://me.test".into()],
            service: Vec::new(),
        };
        assert!(document.pds().is_none());
    }

    #[test]
    fn a_callback_gives_up_its_code_when_state_and_issuer_both_match() {
        let code = read_callback(
            "io.github.x:/callback?code=abc&state=xyz&iss=https://bsky.social",
            "xyz",
            "https://bsky.social",
        )
        .expect("code");
        assert_eq!(code, "abc");
    }

    #[test]
    fn a_mismatched_state_aborts() {
        let err = read_callback(
            "io.github.x:/callback?code=abc&state=WRONG&iss=https://bsky.social",
            "xyz",
            "https://bsky.social",
        )
        .expect_err("state mismatch");
        assert!(matches!(err, AppError::Unauthorized(_)), "{err}");
    }

    #[test]
    fn a_callback_from_another_issuer_is_refused() {
        let err = read_callback(
            "io.github.x:/callback?code=abc&state=xyz&iss=https://evil.test",
            "xyz",
            "https://bsky.social",
        )
        .expect_err("issuer mismatch");
        assert!(err.to_string().contains("evil.test"), "{err}");
    }

    #[test]
    fn a_callback_with_no_issuer_is_refused() {
        assert!(
            read_callback(
                "io.github.x:/callback?code=abc&state=xyz",
                "xyz",
                "https://bsky.social"
            )
            .is_err(),
            "this profile requires `iss` on every callback"
        );
    }

    #[test]
    fn a_cancelled_consent_screen_reads_as_cancellation() {
        let err = read_callback(
            "io.github.x:/callback?error=access_denied&state=xyz",
            "xyz",
            "https://bsky.social",
        )
        .expect_err("cancelled");
        assert!(err.to_string().contains("cancelled"), "{err}");
    }

    #[test]
    fn an_expiry_is_computed_only_when_the_server_gave_a_lifetime() {
        let with = TokenResponse {
            access_token: "a".into(),
            refresh_token: None,
            expires_in: Some(3600),
            scope: None,
            sub: "did:plc:x".into(),
        };
        assert!(with.expires_at().is_some());

        let without = TokenResponse {
            expires_in: None,
            ..with
        };
        assert!(without.expires_at().is_none());
    }

    /// One test for the process-wide pending slot: tests run in parallel, and
    /// two of them arming and disarming it would race each other.
    #[test]
    fn only_a_callback_for_the_waiting_sign_in_is_delivered() {
        assert!(
            !deliver_callback("io.github.x:/callback?code=stray&state=s1"),
            "a link arriving outside a flow must not be queued for the next one"
        );

        let listener = CallbackListener::arm("s1");
        assert!(
            !deliver_callback("io.github.x:/callback?error=access_denied&state=other"),
            "a callback for another sign-in must not be able to cancel this one"
        );
        assert!(!deliver_callback("io.github.x:/callback?code=abc"));
        assert!(
            listener.receiver.try_recv().is_err(),
            "nothing foreign reaches the flow"
        );

        let ours = "io.github.x:/callback?code=abc&state=s1&iss=https://bsky.social";
        assert!(deliver_callback(ours));
        assert_eq!(listener.wait(Duration::from_secs(1)).expect("ours"), ours);

        drop(listener);
        assert!(
            !deliver_callback(ours),
            "a finished flow is disarmed, so a replay goes nowhere"
        );
    }

    /// Hits the real, public, unauthenticated discovery chain. Ignored by
    /// default so the suite stays offline; run with `--ignored` to re-verify
    /// against Bluesky itself.
    #[test]
    #[ignore = "network"]
    fn the_live_discovery_chain_resolves_a_real_handle() {
        let identity = resolve_identity("bsky.app").expect("resolve");
        assert!(identity.did.starts_with("did:plc:"));
        assert!(identity.pds.starts_with("https://"));

        let server = discover_auth_server(&identity.pds).expect("discover");
        assert_eq!(server.issuer, "https://bsky.social");
        assert!(server.pushed_authorization_request_endpoint.is_some());
    }
}
