//! Standalone corporate mailbox core.
//!
//! This crate is deliberately pure: no sockets, no database, no object storage,
//! and no logging. It owns the security-critical value objects and SMTP
//! transaction state machine that every outer listener/adapter must use so the
//! product cannot accidentally become an open relay.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

const MAX_DOMAIN_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;
const MAX_LOCAL_PART_LEN: usize = 64;
const MAX_ADDRESS_LEN: usize = 320;
const DEFAULT_MAX_RECIPIENTS: usize = 50;
const DEFAULT_MAX_MESSAGE_BYTES: usize = 25 * 1024 * 1024;
const MAX_COMMAND_LINE_BYTES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MailboxError {
    #[error("domain is invalid")]
    InvalidDomain,
    #[error("local part is invalid")]
    InvalidLocalPart,
    #[error("mailbox address is invalid")]
    InvalidAddress,
    #[error("smtp command is invalid")]
    InvalidSmtpCommand,
}

/// Lowercase DNS domain accepted for hosted corporate mail.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DomainName(String);

impl DomainName {
    pub fn parse(input: &str) -> Result<Self, MailboxError> {
        let normalized = input.trim().trim_end_matches('.').to_ascii_lowercase();
        if normalized.is_empty()
            || normalized.len() > MAX_DOMAIN_LEN
            || !normalized.contains('.')
            || !normalized.is_ascii()
        {
            return Err(MailboxError::InvalidDomain);
        }

        for label in normalized.split('.') {
            if !valid_domain_label(label) {
                return Err(MailboxError::InvalidDomain);
            }
        }

        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for DomainName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn valid_domain_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_LABEL_LEN
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Corporate mailbox/alias local-part.
///
/// This intentionally models the local parts we host, not every legal RFC 5321
/// reverse-path. Hosted corporate addresses stay predictable for policy, HR,
/// aliases, and admin UI; remote `MAIL FROM` values are handled separately.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LocalPart(String);

impl LocalPart {
    pub fn parse(input: &str) -> Result<Self, MailboxError> {
        let normalized = input.trim().to_ascii_lowercase();
        if normalized.is_empty()
            || normalized.len() > MAX_LOCAL_PART_LEN
            || !normalized.is_ascii()
            || normalized.starts_with('.')
            || normalized.ends_with('.')
            || normalized.contains("..")
            || !normalized.bytes().all(valid_local_part_byte)
        {
            return Err(MailboxError::InvalidLocalPart);
        }
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for LocalPart {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn valid_local_part_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-' | b'+')
}

/// A hosted mailbox address under our corporate domain policy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MailboxAddress {
    local_part: LocalPart,
    domain: DomainName,
}

impl MailboxAddress {
    pub fn parse(input: &str) -> Result<Self, MailboxError> {
        let value = input.trim().trim_matches('<').trim_matches('>');
        if value.len() > MAX_ADDRESS_LEN {
            return Err(MailboxError::InvalidAddress);
        }
        let Some((local, domain)) = value.rsplit_once('@') else {
            return Err(MailboxError::InvalidAddress);
        };
        Ok(Self {
            local_part: LocalPart::parse(local)?,
            domain: DomainName::parse(domain)?,
        })
    }

    #[must_use]
    pub fn new(local_part: LocalPart, domain: DomainName) -> Self {
        Self { local_part, domain }
    }

    #[must_use]
    pub fn local_part(&self) -> &LocalPart {
        &self.local_part
    }

    #[must_use]
    pub fn domain(&self) -> &DomainName {
        &self.domain
    }

    #[must_use]
    pub fn as_normalized(&self) -> String {
        format!("{}@{}", self.local_part, self.domain)
    }
}

impl Display for MailboxAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.local_part, self.domain)
    }
}

/// Snapshot of hosted domains and recipients used to make SMTP RCPT decisions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MailboxDirectory {
    hosted_domains: BTreeSet<DomainName>,
    recipients: BTreeSet<MailboxAddress>,
}

impl MailboxDirectory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_domain(&mut self, domain: DomainName) {
        self.hosted_domains.insert(domain);
    }

    pub fn add_recipient(&mut self, address: MailboxAddress) {
        self.hosted_domains.insert(address.domain().clone());
        self.recipients.insert(address);
    }

    #[must_use]
    pub fn resolve_recipient(&self, address: &MailboxAddress) -> RecipientResolution {
        if !self.hosted_domains.contains(address.domain()) {
            return RecipientResolution::RelayDenied;
        }
        if self.recipients.contains(address) {
            RecipientResolution::Accepted
        } else {
            RecipientResolution::UnknownLocalRecipient
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecipientResolution {
    Accepted,
    UnknownLocalRecipient,
    RelayDenied,
}

/// SMTP reverse-path. `<>` is valid for bounces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReversePath {
    Null,
    Address(String),
}

/// Completed message accepted by the SMTP state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedMessage {
    pub mail_from: ReversePath,
    pub recipients: Vec<MailboxAddress>,
    pub raw_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpReply {
    pub code: u16,
    pub enhanced_code: Option<&'static str>,
    pub text: String,
}

impl SmtpReply {
    #[must_use]
    pub fn new(code: u16, enhanced_code: Option<&'static str>, text: impl Into<String>) -> Self {
        Self {
            code,
            enhanced_code,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmtpOutput {
    pub replies: Vec<SmtpReply>,
    pub completed: Option<CompletedMessage>,
    pub closed: bool,
}

impl SmtpOutput {
    fn reply(reply: SmtpReply, closed: bool) -> Self {
        Self {
            replies: vec![reply],
            completed: None,
            closed,
        }
    }

    fn replies(replies: Vec<SmtpReply>) -> Self {
        Self {
            replies,
            completed: None,
            closed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpSessionConfig {
    pub hostname: String,
    pub max_recipients: usize,
    pub max_message_bytes: usize,
}

impl Default for SmtpSessionConfig {
    fn default() -> Self {
        Self {
            hostname: "console-mailbox.local".to_owned(),
            max_recipients: DEFAULT_MAX_RECIPIENTS,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmtpMode {
    Command,
    Data,
}

/// Single SMTP transaction state machine. It is intentionally line-oriented so
/// the network listener can own TLS/socket concerns while this core owns SMTP
/// ordering, relay rejection, recipient limits, and DATA completion behavior.
#[derive(Debug, Clone)]
pub struct SmtpSession {
    directory: MailboxDirectory,
    config: SmtpSessionConfig,
    greeted: bool,
    mode: SmtpMode,
    mail_from: Option<ReversePath>,
    recipients: Vec<MailboxAddress>,
    data: Vec<u8>,
    data_over_limit: bool,
    closed: bool,
}

impl SmtpSession {
    #[must_use]
    pub fn new(directory: MailboxDirectory, config: SmtpSessionConfig) -> Self {
        Self {
            directory,
            config,
            greeted: false,
            mode: SmtpMode::Command,
            mail_from: None,
            recipients: Vec::new(),
            data: Vec::new(),
            data_over_limit: false,
            closed: false,
        }
    }

    #[must_use]
    pub fn banner(&self) -> SmtpReply {
        SmtpReply::new(
            220,
            Some("2.0.0"),
            format!("{} ESMTP console-mailbox", self.config.hostname),
        )
    }

    pub fn process_line(&mut self, line: &str) -> SmtpOutput {
        if self.closed {
            return SmtpOutput::reply(
                SmtpReply::new(421, Some("4.4.2"), "smtp session already closed"),
                true,
            );
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.len() > MAX_COMMAND_LINE_BYTES && self.mode == SmtpMode::Command {
            return SmtpOutput::reply(
                SmtpReply::new(500, Some("5.5.2"), "command line too long"),
                false,
            );
        }

        match self.mode {
            SmtpMode::Command => self.process_command(line),
            SmtpMode::Data => self.process_data_line(line),
        }
    }

    fn process_command(&mut self, line: &str) -> SmtpOutput {
        let (verb, arg) = split_command(line);
        match verb.as_str() {
            "EHLO" | "HELO" => self.greet(arg),
            "MAIL" => self.mail_from(arg),
            "RCPT" => self.rcpt_to(arg),
            "DATA" => self.data(),
            "RSET" => {
                self.reset_transaction();
                SmtpOutput::reply(SmtpReply::new(250, Some("2.0.0"), "reset ok"), false)
            }
            "NOOP" => SmtpOutput::reply(SmtpReply::new(250, Some("2.0.0"), "ok"), false),
            "QUIT" => {
                self.closed = true;
                SmtpOutput::reply(SmtpReply::new(221, Some("2.0.0"), "bye"), true)
            }
            _ => SmtpOutput::reply(
                SmtpReply::new(500, Some("5.5.1"), "command not recognized"),
                false,
            ),
        }
    }

    fn greet(&mut self, arg: &str) -> SmtpOutput {
        if arg.trim().is_empty() {
            return SmtpOutput::reply(
                SmtpReply::new(501, Some("5.5.2"), "helo name required"),
                false,
            );
        }
        self.greeted = true;
        self.reset_transaction();
        SmtpOutput::replies(vec![
            SmtpReply::new(250, Some("2.0.0"), self.config.hostname.clone()),
            SmtpReply::new(
                250,
                Some("2.0.0"),
                format!("SIZE {}", self.config.max_message_bytes),
            ),
            SmtpReply::new(250, Some("2.0.0"), "8BITMIME"),
        ])
    }

    fn mail_from(&mut self, arg: &str) -> SmtpOutput {
        if !self.greeted {
            return bad_sequence("send EHLO/HELO first");
        }
        let Some(path_arg) = arg.trim().strip_prefix_case_insensitive("FROM:") else {
            return SmtpOutput::reply(
                SmtpReply::new(501, Some("5.5.2"), "MAIL requires FROM:<address>"),
                false,
            );
        };
        let Ok(path) = parse_reverse_path(path_arg) else {
            return SmtpOutput::reply(
                SmtpReply::new(501, Some("5.1.7"), "invalid reverse path"),
                false,
            );
        };
        self.reset_transaction();
        self.mail_from = Some(path);
        SmtpOutput::reply(SmtpReply::new(250, Some("2.1.0"), "sender ok"), false)
    }

    fn rcpt_to(&mut self, arg: &str) -> SmtpOutput {
        if self.mail_from.is_none() {
            return bad_sequence("send MAIL FROM first");
        }
        if self.recipients.len() >= self.config.max_recipients {
            return SmtpOutput::reply(
                SmtpReply::new(452, Some("4.5.3"), "too many recipients"),
                false,
            );
        }
        let Some(path_arg) = arg.trim().strip_prefix_case_insensitive("TO:") else {
            return SmtpOutput::reply(
                SmtpReply::new(501, Some("5.5.2"), "RCPT requires TO:<address>"),
                false,
            );
        };
        let Ok(address) = parse_rcpt_path(path_arg) else {
            return SmtpOutput::reply(
                SmtpReply::new(501, Some("5.1.3"), "invalid recipient address"),
                false,
            );
        };
        match self.directory.resolve_recipient(&address) {
            RecipientResolution::Accepted => {
                self.recipients.push(address);
                SmtpOutput::reply(SmtpReply::new(250, Some("2.1.5"), "recipient ok"), false)
            }
            RecipientResolution::UnknownLocalRecipient => SmtpOutput::reply(
                SmtpReply::new(550, Some("5.1.1"), "unknown local recipient"),
                false,
            ),
            RecipientResolution::RelayDenied => {
                SmtpOutput::reply(SmtpReply::new(550, Some("5.7.1"), "relay denied"), false)
            }
        }
    }

    fn data(&mut self) -> SmtpOutput {
        if self.recipients.is_empty() {
            return bad_sequence("send RCPT TO first");
        }
        self.mode = SmtpMode::Data;
        self.data.clear();
        self.data_over_limit = false;
        SmtpOutput::reply(
            SmtpReply::new(354, Some("3.0.0"), "end data with <CR><LF>.<CR><LF>"),
            false,
        )
    }

    fn process_data_line(&mut self, line: &str) -> SmtpOutput {
        if line == "." {
            self.mode = SmtpMode::Command;
            if self.data_over_limit {
                self.reset_transaction();
                return SmtpOutput::reply(
                    SmtpReply::new(552, Some("5.3.4"), "message size exceeds fixed limit"),
                    false,
                );
            }

            let Some(mail_from) = self.mail_from.take() else {
                self.reset_transaction();
                return bad_sequence("send MAIL FROM first");
            };
            let completed = CompletedMessage {
                mail_from,
                recipients: std::mem::take(&mut self.recipients),
                raw_data: std::mem::take(&mut self.data),
            };
            self.reset_transaction();
            return SmtpOutput {
                replies: vec![SmtpReply::new(250, Some("2.0.0"), "message accepted")],
                completed: Some(completed),
                closed: false,
            };
        }

        let data_line = line.strip_prefix('.').unwrap_or(line);
        let incoming_len = data_line.len().saturating_add(2);
        if self.data.len().saturating_add(incoming_len) > self.config.max_message_bytes {
            self.data_over_limit = true;
            return SmtpOutput::default();
        }
        self.data.extend_from_slice(data_line.as_bytes());
        self.data.extend_from_slice(b"\r\n");
        SmtpOutput::default()
    }

    fn reset_transaction(&mut self) {
        self.mode = SmtpMode::Command;
        self.mail_from = None;
        self.recipients.clear();
        self.data.clear();
        self.data_over_limit = false;
    }
}

fn split_command(line: &str) -> (String, &str) {
    let trimmed = line.trim_start();
    if let Some((verb, arg)) = trimmed.split_once(char::is_whitespace) {
        (verb.to_ascii_uppercase(), arg.trim_start())
    } else {
        (trimmed.to_ascii_uppercase(), "")
    }
}

fn bad_sequence(text: &'static str) -> SmtpOutput {
    SmtpOutput::reply(SmtpReply::new(503, Some("5.5.1"), text), false)
}

trait StripPrefixAsciiCase {
    fn strip_prefix_case_insensitive<'a>(&'a self, prefix: &str) -> Option<&'a str>;
}

impl StripPrefixAsciiCase for str {
    fn strip_prefix_case_insensitive<'a>(&'a self, prefix: &str) -> Option<&'a str> {
        if self.len() < prefix.len() {
            return None;
        }
        let (head, tail) = self.split_at(prefix.len());
        if head.eq_ignore_ascii_case(prefix) {
            Some(tail.trim_start())
        } else {
            None
        }
    }
}

fn parse_reverse_path(arg: &str) -> Result<ReversePath, MailboxError> {
    let path = extract_angle_path(arg)?;
    if path.is_empty() {
        return Ok(ReversePath::Null);
    }
    if path.len() > MAX_ADDRESS_LEN || !path.contains('@') {
        return Err(MailboxError::InvalidAddress);
    }
    Ok(ReversePath::Address(path.to_owned()))
}

fn parse_rcpt_path(arg: &str) -> Result<MailboxAddress, MailboxError> {
    MailboxAddress::parse(extract_angle_path(arg)?)
}

fn extract_angle_path(arg: &str) -> Result<&str, MailboxError> {
    let trimmed = arg.trim_start();
    let Some(rest) = trimmed.strip_prefix('<') else {
        return Err(MailboxError::InvalidSmtpCommand);
    };
    let Some((path, _params)) = rest.split_once('>') else {
        return Err(MailboxError::InvalidSmtpCommand);
    };
    Ok(path.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> MailboxDirectory {
        let mut directory = MailboxDirectory::new();
        directory.add_recipient(MailboxAddress::parse("dispatcher@knllogistic.com").unwrap());
        directory.add_recipient(MailboxAddress::parse("hr@cossok.com").unwrap());
        directory
    }

    fn session() -> SmtpSession {
        SmtpSession::new(
            directory(),
            SmtpSessionConfig {
                hostname: "mail.knllogistic.com".to_owned(),
                max_recipients: 2,
                max_message_bytes: 128,
            },
        )
    }

    #[test]
    fn hosted_domain_and_local_part_are_normalized_and_validated() {
        assert_eq!(
            DomainName::parse("KNLLOGISTIC.COM.").unwrap().as_str(),
            "knllogistic.com"
        );
        assert_eq!(
            LocalPart::parse("Dispatch.Team+Night").unwrap().as_str(),
            "dispatch.team+night"
        );
        assert!(DomainName::parse("localhost").is_err());
        assert!(DomainName::parse("-bad.example").is_err());
        assert!(LocalPart::parse("bad..dots").is_err());
        assert!(LocalPart::parse("bad space").is_err());
    }

    #[test]
    fn directory_accepts_only_known_local_recipients_and_rejects_relay() {
        let accepted = MailboxAddress::parse("dispatcher@knllogistic.com").unwrap();
        let unknown = MailboxAddress::parse("missing@knllogistic.com").unwrap();
        let relay = MailboxAddress::parse("person@example.com").unwrap();
        let directory = directory();

        assert_eq!(
            directory.resolve_recipient(&accepted),
            RecipientResolution::Accepted
        );
        assert_eq!(
            directory.resolve_recipient(&unknown),
            RecipientResolution::UnknownLocalRecipient
        );
        assert_eq!(
            directory.resolve_recipient(&relay),
            RecipientResolution::RelayDenied
        );
    }

    #[test]
    fn smtp_accepts_local_delivery_and_completes_on_dot() {
        let mut smtp = session();
        assert_eq!(smtp.banner().code, 220);
        assert_eq!(
            smtp.process_line("EHLO sender.example").replies[0].code,
            250
        );
        assert_eq!(
            smtp.process_line("MAIL FROM:<operator@example.com> SIZE=42")
                .replies[0]
                .code,
            250
        );
        assert_eq!(
            smtp.process_line("RCPT TO:<dispatcher@knllogistic.com>")
                .replies[0]
                .code,
            250
        );
        assert_eq!(smtp.process_line("DATA").replies[0].code, 354);
        assert!(smtp.process_line("Subject: Dispatch").replies.is_empty());
        assert!(smtp.process_line("").replies.is_empty());
        assert!(smtp.process_line("Body").replies.is_empty());
        let output = smtp.process_line(".");

        let completed = output.completed.expect("message should complete");
        assert_eq!(output.replies[0].code, 250);
        assert_eq!(
            completed.recipients[0].as_normalized(),
            "dispatcher@knllogistic.com"
        );
        assert_eq!(completed.raw_data, b"Subject: Dispatch\r\n\r\nBody\r\n");
    }

    #[test]
    fn smtp_rejects_relay_and_unknown_local_recipients() {
        let mut smtp = session();
        smtp.process_line("EHLO sender.example");
        smtp.process_line("MAIL FROM:<operator@example.com>");

        let relay = smtp.process_line("RCPT TO:<person@example.com>");
        assert_eq!(relay.replies[0].code, 550);
        assert_eq!(relay.replies[0].enhanced_code, Some("5.7.1"));

        let unknown = smtp.process_line("RCPT TO:<missing@knllogistic.com>");
        assert_eq!(unknown.replies[0].code, 550);
        assert_eq!(unknown.replies[0].enhanced_code, Some("5.1.1"));
    }

    #[test]
    fn smtp_enforces_command_order_and_recipient_limit() {
        let mut smtp = session();
        assert_eq!(smtp.process_line("DATA").replies[0].code, 503);
        smtp.process_line("EHLO sender.example");
        assert_eq!(
            smtp.process_line("RCPT TO:<dispatcher@knllogistic.com>")
                .replies[0]
                .code,
            503
        );
        smtp.process_line("MAIL FROM:<operator@example.com>");
        assert_eq!(
            smtp.process_line("RCPT TO:<dispatcher@knllogistic.com>")
                .replies[0]
                .code,
            250
        );
        assert_eq!(
            smtp.process_line("RCPT TO:<hr@cossok.com>").replies[0].code,
            250
        );
        assert_eq!(
            smtp.process_line("RCPT TO:<hr@cossok.com>").replies[0].code,
            452
        );
    }

    #[test]
    fn smtp_data_mode_dot_unstuffs_and_defers_size_error_until_final_dot() {
        let mut smtp = session();
        smtp.process_line("EHLO sender.example");
        smtp.process_line("MAIL FROM:<operator@example.com>");
        smtp.process_line("RCPT TO:<dispatcher@knllogistic.com>");
        smtp.process_line("DATA");
        smtp.process_line("..literal dot");
        let output = smtp.process_line(".");
        assert_eq!(
            output.completed.expect("message should complete").raw_data,
            b".literal dot\r\n"
        );

        smtp.process_line("MAIL FROM:<operator@example.com>");
        smtp.process_line("RCPT TO:<dispatcher@knllogistic.com>");
        smtp.process_line("DATA");
        let long_line = "x".repeat(200);
        assert!(smtp.process_line(&long_line).replies.is_empty());
        let rejected = smtp.process_line(".");
        assert_eq!(rejected.replies[0].code, 552);
        assert!(rejected.completed.is_none());
    }

    #[test]
    fn smtp_rset_clears_transaction_without_losing_greeting() {
        let mut smtp = session();
        smtp.process_line("EHLO sender.example");
        smtp.process_line("MAIL FROM:<operator@example.com>");
        smtp.process_line("RCPT TO:<dispatcher@knllogistic.com>");
        assert_eq!(smtp.process_line("RSET").replies[0].code, 250);
        assert_eq!(smtp.process_line("DATA").replies[0].code, 503);
        assert_eq!(
            smtp.process_line("MAIL FROM:<operator@example.com>")
                .replies[0]
                .code,
            250
        );
    }
}
