//! QR-code login through Discord's remote-auth gateway: the phone app scans a code and approves the
//! sign-in, and the token is handed over encrypted to a key only this client holds.

use std::future::Future;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::time::{interval_at, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::rest::{ApiError, Captcha, Rest};

const URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";

#[derive(Debug, Clone)]
pub enum RemoteAuthEvent {
    /// Show this URL as a QR code.
    QrCode(String),
    /// Scanned; waiting for the user to approve on their phone.
    Scanned { username: String },
    /// Discord wants a captcha solved before it hands over the token; the solver has been asked.
    Captcha,
    Token(String),
    /// The user cancelled on their phone, or the code expired; a new one can be requested.
    Cancelled,
    Failed(String),
}

#[derive(Deserialize)]
struct Msg {
    op: String,
    #[serde(default)]
    heartbeat_interval: Option<u64>,
    #[serde(default)]
    encrypted_nonce: Option<String>,
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    encrypted_user_payload: Option<String>,
    #[serde(default)]
    ticket: Option<String>,
}

/// Runs one remote-auth session, reporting progress on `tx` until a token arrives or the session ends.
/// `solve` answers a captcha (the hCaptcha response token), or returns `None` if the user gave up.
pub async fn run<F, Fut>(tx: mpsc::UnboundedSender<RemoteAuthEvent>, solve: F)
where
    F: Fn(Captcha) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let result = session(&tx, solve).await;
    if let Err(e) = result {
        let _ = tx.send(RemoteAuthEvent::Failed(e.to_string()));
    }
}

use crate::rsa_key::{BoxErr, KeyPair};

async fn session<F, Fut>(tx: &mpsc::UnboundedSender<RemoteAuthEvent>, solve: F) -> Result<(), BoxErr>
where
    F: Fn(Captcha) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let key = tokio::task::spawn_blocking(KeyPair::generate).await??;
    let public_der = key.public_key_der()?;
    let decrypt = |b64: &str| -> Result<Vec<u8>, BoxErr> {
        key.decrypt(&STANDARD.decode(b64)?)
    };

    let mut request = URL.into_client_request()?;
    // The gateway rejects connections that don't come from Discord's web origin.
    request.headers_mut().insert("Origin", "https://discord.com".parse()?);
    request.headers_mut().insert("User-Agent", crate::identity::user_agent().parse()?);
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await?;

    let mut heartbeat = interval_at(Instant::now() + Duration::from_secs(3600), Duration::from_secs(3600));
    loop {
        tokio::select! {
            frame = ws.next() => {
                let text = match frame {
                    Some(Ok(WsMessage::Text(t))) => t,
                    Some(Ok(WsMessage::Close(_))) | None => {
                        let _ = tx.send(RemoteAuthEvent::Cancelled);
                        return Ok(());
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.into()),
                };
                let msg: Msg = serde_json::from_str(&text)?;
                match msg.op.as_str() {
                    "hello" => {
                        let iv = Duration::from_millis(msg.heartbeat_interval.unwrap_or(41_250));
                        heartbeat = interval_at(Instant::now() + iv, iv);
                        let init = json!({ "op": "init", "encoded_public_key": STANDARD.encode(&public_der) });
                        ws.send(WsMessage::text(init.to_string())).await?;
                    }
                    "nonce_proof" => {
                        let nonce = decrypt(msg.encrypted_nonce.as_deref().unwrap_or_default())?;
                        let proof = URL_SAFE_NO_PAD.encode(Sha256::digest(&nonce));
                        ws.send(WsMessage::text(json!({ "op": "nonce_proof", "proof": proof }).to_string())).await?;
                    }
                    "pending_remote_init" => {
                        let fp = msg.fingerprint.unwrap_or_default();
                        let _ = tx.send(RemoteAuthEvent::QrCode(format!("https://discord.com/ra/{fp}")));
                    }
                    "pending_ticket" => {
                        // "id:discriminator:avatar:username"
                        let payload = decrypt(msg.encrypted_user_payload.as_deref().unwrap_or_default())?;
                        let payload = String::from_utf8_lossy(&payload);
                        let username = payload.splitn(4, ':').nth(3).unwrap_or("").to_owned();
                        let _ = tx.send(RemoteAuthEvent::Scanned { username });
                    }
                    "pending_login" => {
                        let ticket = msg.ticket.unwrap_or_default();
                        let mut rest = Rest::new("");
                        let encrypted = loop {
                            match rest.remote_auth_login(&ticket).await {
                                Err(ApiError::CaptchaRequired(captcha)) => {
                                    let _ = tx.send(RemoteAuthEvent::Captcha);
                                    let Some(answer) = solve(captcha.clone()).await else {
                                        let _ = tx.send(RemoteAuthEvent::Cancelled);
                                        return Ok(());
                                    };
                                    rest = Rest::with_captcha("", &captcha, &answer);
                                }
                                result => break result?,
                            }
                        };
                        let token = String::from_utf8(decrypt(&encrypted)?)?;
                        let _ = tx.send(RemoteAuthEvent::Token(token));
                        let _ = ws.close(None).await;
                        return Ok(());
                    }
                    "cancel" => {
                        let _ = tx.send(RemoteAuthEvent::Cancelled);
                        return Ok(());
                    }
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                ws.send(WsMessage::text(r#"{"op":"heartbeat"}"#)).await?;
            }
            _ = tx.closed() => return Ok(()),
        }
    }
}
