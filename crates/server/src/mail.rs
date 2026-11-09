//! 메일 발송(응시 배정 초대). 발송은 [`Mailer`] 트레잇 뒤에 있다 — 시험은 가짜 발송기, 운영은 [`SmtpMailer`].
//!
//! Sandcorp 다른 저장소에는 메일 발송이 없어(알림은 Discord·문자) 제공자에 매이지 않는 SMTP를 쓴다.
//! 설정(환경변수, [`from_env`]):
//!
//! | 키 | 뜻 |
//! |---|---|
//! | `PROMPTLAB_SMTP_HOST` | SMTP 서버. 없으면 발송이 꺼진다([`Disabled`] — 초대는 만들어지고 링크 복사로 쓴다) |
//! | `PROMPTLAB_SMTP_PORT` | 기본 `465`(암묵 TLS). `587`이면 STARTTLS |
//! | `PROMPTLAB_SMTP_USERNAME`·`PROMPTLAB_SMTP_PASSWORD` | SMTP 인증(비밀 — 로그에 찍지 않는다) |
//! | `PROMPTLAB_MAIL_FROM` | 보낸이(`promptlab <no-reply@sandcorp.co.kr>` 꼴). HOST가 있으면 필수 |
//!
//! 발송 실패는 초대 생성을 막지 않는다 — 초대마다 [`Delivery`](promptlab_shared::exam::Delivery)로 남는다.

use std::time::Duration;

use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::auth::token::BoxFuture;

/// 보낼 메일 하나(본문은 평문).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MailError {
    /// 메일 설정이 없다.
    #[error("메일 발송이 설정되지 않았다")]
    NotConfigured,
    /// 보내지 못했다. 메시지에는 비밀이 없다(주소·서버 응답 요약).
    #[error("메일을 보내지 못했다: {0}")]
    Failed(String),
}

/// 메일 발송기.
pub trait Mailer: Send + Sync {
    fn send<'a>(&'a self, mail: &'a Mail) -> BoxFuture<'a, Result<(), MailError>>;
}

/// 설정이 없을 때: 언제나 [`MailError::NotConfigured`].
pub struct Disabled;

impl Mailer for Disabled {
    fn send<'a>(&'a self, _mail: &'a Mail) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async { Err(MailError::NotConfigured) })
    }
}

/// SMTP 발송기(rustls).
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

/// SMTP 설정. 비밀번호는 `Debug`에 찍지 않는다.
#[derive(Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "…"))
            .field("from", &self.from)
            .finish()
    }
}

impl SmtpMailer {
    pub fn new(config: &SmtpConfig) -> Result<Self, String> {
        let from: Mailbox = config
            .from
            .parse()
            .map_err(|e| format!("PROMPTLAB_MAIL_FROM 이 주소 꼴이 아니다: {e}"))?;
        let builder = if config.port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
        }
        .map_err(|e| format!("SMTP 설정이 틀렸다: {e}"))?
        .port(config.port)
        .timeout(Some(Duration::from_secs(15)));
        let builder = match (&config.username, &config.password) {
            (Some(user), Some(password)) => {
                builder.credentials(Credentials::new(user.clone(), password.clone()))
            }
            _ => builder,
        };
        Ok(Self {
            transport: builder.build(),
            from,
        })
    }
}

impl Mailer for SmtpMailer {
    fn send<'a>(&'a self, mail: &'a Mail) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            let to: Mailbox = mail
                .to
                .parse()
                .map_err(|e| MailError::Failed(format!("받는 주소가 틀렸다: {e}")))?;
            let message = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(&mail.subject)
                .header(ContentType::TEXT_PLAIN)
                .body(mail.body.clone())
                .map_err(|e| MailError::Failed(e.to_string()))?;
            self.transport
                .send(message)
                .await
                .map(|_| ())
                .map_err(|e| MailError::Failed(e.to_string()))
        })
    }
}

/// 환경변수에서 SMTP 설정을 읽는다. HOST가 없으면 `Ok(None)`(발송 꺼짐).
pub fn config_from(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<SmtpConfig>, String> {
    let get = |k: &str| lookup(k).filter(|v| !v.trim().is_empty());
    let Some(host) = get("PROMPTLAB_SMTP_HOST") else {
        return Ok(None);
    };
    let port = match get("PROMPTLAB_SMTP_PORT") {
        Some(p) => p
            .trim()
            .parse()
            .map_err(|_| format!("PROMPTLAB_SMTP_PORT 가 숫자가 아니다: {p}"))?,
        None => 465,
    };
    let from = get("PROMPTLAB_MAIL_FROM")
        .ok_or("PROMPTLAB_SMTP_HOST 가 있으면 PROMPTLAB_MAIL_FROM 도 있어야 한다")?;
    Ok(Some(SmtpConfig {
        host: host.trim().to_owned(),
        port,
        username: get("PROMPTLAB_SMTP_USERNAME"),
        password: get("PROMPTLAB_SMTP_PASSWORD"),
        from,
    }))
}

/// 응시 배정 초대 메일.
pub fn exam_invite(
    to: &str,
    org_name: &str,
    exam_title: &str,
    link: &str,
    expires_days: i64,
) -> Mail {
    Mail {
        to: to.to_owned(),
        subject: format!("[promptlab] {org_name} — 「{exam_title}」 응시 안내"),
        body: format!(
            "{org_name}에서 promptlab 시험 「{exam_title}」에 초대했습니다.\n\n\
             1. 아래 링크를 열고 카카오로 로그인하면 시험이 배정됩니다.\n   {link}\n\
             2. promptlab 응시 앱을 열어 같은 카카오 계정으로 로그인하면 배정된 시험이 보입니다.\n\n\
             링크는 {expires_days}일 동안 쓸 수 있고, 처음 들어온 한 사람에게만 배정됩니다. 다른 사람에게 전달하지 마세요.\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 호스트가_없으면_발송이_꺼지고_있으면_보낸이가_필요하다() {
        assert_eq!(config_from(|_| None), Ok(None));
        let only_host =
            |k: &str| (k == "PROMPTLAB_SMTP_HOST").then(|| "smtp.example.com".to_owned());
        assert!(config_from(only_host).is_err());
        let full = |k: &str| match k {
            "PROMPTLAB_SMTP_HOST" => Some("smtp.example.com".to_owned()),
            "PROMPTLAB_SMTP_PORT" => Some("587".to_owned()),
            "PROMPTLAB_SMTP_PASSWORD" => Some("비밀값".to_owned()),
            "PROMPTLAB_MAIL_FROM" => Some("promptlab <no-reply@example.com>".to_owned()),
            _ => None,
        };
        let config = config_from(full).unwrap().unwrap();
        assert_eq!(config.port, 587);
        assert!(
            !format!("{config:?}").contains("비밀값"),
            "Debug 에 비밀번호가 찍혔다"
        );
        assert!(SmtpMailer::new(&config).is_ok());
    }
}
