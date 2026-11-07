//! The Classifier (ADR-0006, PLAN.md §5.1): Selection metadata in,
//! Sensitivity out.
//!
//! It decides in two steps. [`Classifier::offer`] looks only at what the
//! source advertised and who had focus, before a byte is read: a Secret is
//! never read at all. [`Classifier::content`] looks at what was read, for
//! text that looks like a secret.
//!
//! Patterns run on the bytes as read (`regex::bytes`), so no extra copy of a
//! secret is ever made just to match it.

use hoarder_config::Privacy;
use hoarder_ipc::Sensitivity;
use regex::bytes::{Regex, RegexSet};

use crate::clipboard::PASSWORD_HINT;

/// Built-in secret patterns (PLAN.md §5.1). Card numbers are found by
/// [`CARD`] and confirmed with Luhn, which a regex cannot do.
const BUILT_IN: [&str; 8] = [
    // PEM private keys: RSA, EC, DSA, OpenSSH, PKCS#8 (encrypted or not), PGP.
    r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----",
    // AWS access key ids: long-term (AKIA) and temporary (ASIA).
    r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
    // GCP service-account JSON.
    r#""type"\s*:\s*"service_account""#,
    // GitHub tokens: classic and fine-grained.
    r"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
    r"\bgithub_pat_[A-Za-z0-9_]{22,}\b",
    // Slack tokens.
    r"\bxox[abpr]-[A-Za-z0-9-]{10,}",
    // Stripe live secret keys.
    r"\bsk_live_[A-Za-z0-9]{16,}",
    // JWT: a base64url `{"` header and payload, and a signature.
    r"\beyJ[A-Za-z0-9_-]{5,}\.eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{10,}",
];

/// A card-number candidate: 13–19 digits, optionally grouped by single
/// spaces or dashes, starting with a payment-network digit (2–6), so that
/// millisecond timestamps and the like are not mistaken for cards.
const CARD: &str = r"(?:^|[^0-9])([2-6](?:[ -]?[0-9]){12,18})(?:[^0-9]|$)";

/// Decides the Sensitivity of each Selection change.
#[derive(Debug)]
pub struct Classifier {
    exclude_apps: Vec<String>,
    patterns: RegexSet,
    card: Option<Regex>,
}

impl Classifier {
    /// A Classifier for `privacy`, plus a warning for each `extra_patterns`
    /// entry that is not a valid regex (it is skipped).
    pub fn new(privacy: &Privacy) -> (Classifier, Vec<String>) {
        let mut warnings = Vec::new();
        let mut patterns: Vec<&str> = Vec::new();
        if privacy.patterns {
            patterns.extend(BUILT_IN);
        }
        for extra in &privacy.extra_patterns {
            match Regex::new(extra) {
                Ok(_) => patterns.push(extra),
                Err(e) => warnings.push(format!("privacy.extra_patterns: skipping {extra:?}: {e}")),
            }
        }
        let patterns = RegexSet::new(&patterns).expect("every pattern was checked");
        let card = privacy
            .patterns
            .then(|| Regex::new(CARD).expect("the card pattern is valid"));
        let classifier = Classifier {
            exclude_apps: privacy.exclude_apps.clone(),
            patterns,
            card,
        };
        (classifier, warnings)
    }

    /// The Sensitivity known from the Offer alone, before anything is
    /// read: `Secret` if a password manager marked it, `Concealed` if an
    /// excluded app had focus, `None` if it depends on the content.
    pub fn offer(&self, mime_types: &[String], focused_app: Option<&str>) -> Option<Sensitivity> {
        if mime_types.iter().any(|m| m == PASSWORD_HINT) {
            return Some(Sensitivity::Secret);
        }
        let excluded = focused_app.is_some_and(|app| {
            self.exclude_apps
                .iter()
                .any(|ex| ex.eq_ignore_ascii_case(app))
        });
        excluded.then_some(Sensitivity::Concealed)
    }

    /// The Sensitivity of what was read: `Concealed` if any text
    /// representation matches a secret pattern, else `Normal`.
    pub fn content<'a>(&self, reprs: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Sensitivity {
        let concealed = reprs
            .into_iter()
            .filter(|(mime, _)| is_text(mime))
            .any(|(_, data)| self.matches(data));
        if concealed {
            Sensitivity::Concealed
        } else {
            Sensitivity::Normal
        }
    }

    fn matches(&self, text: &[u8]) -> bool {
        self.patterns.is_match(text)
            || self.card.as_ref().is_some_and(|card| {
                card.captures_iter(text)
                    .any(|c| luhn(c.get(1).map_or(&[][..], |m| m.as_bytes())))
            })
    }
}

/// Whether a representation is text worth matching.
fn is_text(mime: &str) -> bool {
    let essence = mime.split(';').next().unwrap_or(mime).trim();
    essence.starts_with("text/")
        || matches!(essence, "UTF8_STRING" | "STRING" | "TEXT")
        || essence == "application/json"
        || essence.ends_with("+json")
        || essence == "application/x-pem-file"
}

/// The Luhn checksum over the digits of `number` (separators skipped).
fn luhn(number: &[u8]) -> bool {
    let mut sum = 0u32;
    let mut count = 0usize;
    for (i, d) in number
        .iter()
        .rev()
        .filter(|b| b.is_ascii_digit())
        .map(|b| u32::from(b - b'0'))
        .enumerate()
    {
        count += 1;
        sum += if i % 2 == 1 {
            let x = d * 2;
            if x > 9 { x - 9 } else { x }
        } else {
            d
        };
    }
    (13..=19).contains(&count) && sum.is_multiple_of(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classifier() -> Classifier {
        Classifier::new(&Privacy::default()).0
    }

    fn text(s: &str) -> Sensitivity {
        classifier().content([("text/plain;charset=utf-8", s.as_bytes())])
    }

    // Each built-in pattern: samples that must be Concealed, and near
    // misses that must stay Normal. The tokens are fakes, split with
    // `concat!` so secret scanners do not mistake them for real ones.
    const TABLE: &[(&str, &[&str], &[&str])] = &[
        (
            "PEM private key",
            &[
                "-----BEGIN RSA PRIVATE KEY-----\nMIIEow...\n-----END RSA PRIVATE KEY-----",
                "-----BEGIN PRIVATE KEY-----\nMIIE",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1r",
                "-----BEGIN ENCRYPTED PRIVATE KEY-----",
                "-----BEGIN EC PRIVATE KEY-----",
                "-----BEGIN PGP PRIVATE KEY BLOCK-----",
            ],
            &[
                "-----BEGIN CERTIFICATE-----\nMIID",
                "-----BEGIN PUBLIC KEY-----",
                "-----BEGIN PGP PUBLIC KEY BLOCK-----",
                "my private key is in the drawer",
            ],
        ),
        (
            "AWS access key id",
            &[
                "AKIAIOSFODNN7EXAMPLE",
                "aws_access_key_id = AKIAIOSFODNN7EXAMPLE\n",
                concat!("ASIA", "IOSFODNN7EXAMPLE"),
            ],
            &[
                "AKIA",
                "AKIAIOSFODNN7EXAMPL",
                "XAKIAIOSFODNN7EXAMPLE",
                "akiaiosfodnn7example",
            ],
        ),
        (
            "GCP service-account JSON",
            &[
                r#"{"type": "service_account", "project_id": "p", "private_key_id": "k"}"#,
                "{\n  \"type\":\"service_account\",\n}",
            ],
            &[
                r#"{"type": "authorized_user"}"#,
                "a service_account is a robot",
            ],
        ),
        (
            "GitHub token",
            &[
                "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
                "token: gho_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789\n",
                "ghs_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
                "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz",
            ],
            &[
                "ghp_short",
                "ghx_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
                "the ghp_ prefix marks a personal token",
            ],
        ),
        (
            "Slack token",
            &[
                concat!("xox", "b-123456789012-1234567890123-AbCdEfGhIjKlMnOp"),
                concat!("xox", "p-1234567890-abcdef"),
            ],
            &["xoxz-123456789012-1234567890123", "xoxb-", "xoxo hugs"],
        ),
        (
            "Stripe live key",
            &[
                concat!("sk_", "live_4eC39HqLyjWDarjtT1zdp7dc"),
                concat!("STRIPE=sk_", "live_abcdefghijklmnop"),
            ],
            &[
                concat!("sk_", "test_4eC39HqLyjWDarjtT1zdp7dc"),
                "sk_live_",
                concat!("pk_", "live_4eC39HqLyjWDarjtT1zdp7dc"),
            ],
        ),
        (
            "JWT",
            &[
                "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
                "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJlLWhlcmU",
            ],
            &["eyJhbGciOiJIUzI1NiJ9", "eyJ.eyJ.sig", "version 1.2.3"],
        ),
        (
            "card number",
            &[
                "4111111111111111",
                "4111 1111 1111 1111",
                "5500-0000-0000-0004",
                "Amex: 378282246310005.",
                "6011111111111117",
                "2223003122003222",
            ],
            &[
                // Luhn fails.
                "4111111111111112",
                // Too short / too long.
                "411111111111",
                "41111111111111111111",
                // Not a payment network: a millisecond timestamp passes Luhn.
                "1730937600006",
                // Plain prose and other numbers.
                "call me at +49 30 1234567",
                "order 42",
            ],
        ),
    ];

    #[test]
    fn every_built_in_pattern_conceals_its_samples_and_spares_near_misses() {
        for (name, positives, negatives) in TABLE {
            for sample in *positives {
                assert_eq!(text(sample), Sensitivity::Concealed, "{name}: {sample:?}");
            }
            for sample in *negatives {
                assert_eq!(text(sample), Sensitivity::Normal, "{name}: {sample:?}");
            }
        }
    }

    #[test]
    fn the_timestamp_negative_really_passes_luhn() {
        // Keeps the card table honest: this is excluded by its first digit.
        assert!(luhn(b"1730937600006"));
    }

    #[test]
    fn built_in_patterns_can_be_switched_off() {
        let privacy = Privacy {
            patterns: false,
            ..Privacy::default()
        };
        let (c, _) = Classifier::new(&privacy);
        let ghp = "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";
        assert_eq!(
            c.content([("text/plain", ghp.as_bytes())]),
            Sensitivity::Normal
        );
        assert_eq!(
            c.content([("text/plain", b"4111111111111111".as_slice())]),
            Sensitivity::Normal
        );
    }

    #[test]
    fn extra_patterns_conceal_and_bad_ones_are_reported_not_fatal() {
        let privacy = Privacy {
            patterns: false,
            extra_patterns: vec!["^corp-[0-9]{6}$".into(), "(unclosed".into()],
            ..Privacy::default()
        };
        let (c, warnings) = Classifier::new(&privacy);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("(unclosed"));
        assert_eq!(
            c.content([("text/plain", b"corp-123456".as_slice())]),
            Sensitivity::Concealed
        );
        assert_eq!(
            c.content([("text/plain", b"corp-12345".as_slice())]),
            Sensitivity::Normal
        );
    }

    #[test]
    fn only_text_representations_are_matched() {
        let ghp = b"ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789".as_slice();
        let c = classifier();
        for mime in ["UTF8_STRING", "text/html", "application/json", "text/plain"] {
            assert_eq!(c.content([(mime, ghp)]), Sensitivity::Concealed, "{mime}");
        }
        assert_eq!(c.content([("image/png", ghp)]), Sensitivity::Normal);
        assert_eq!(
            c.content([("image/png", b"x".as_slice()), ("text/plain", ghp)]),
            Sensitivity::Concealed
        );
    }

    #[test]
    fn a_password_manager_hint_is_secret_before_anything_is_read() {
        let types = ["text/plain".to_string(), PASSWORD_HINT.to_string()];
        assert_eq!(classifier().offer(&types, None), Some(Sensitivity::Secret));
        // Even from an excluded app: Secret is the stricter tier.
        assert_eq!(
            classifier().offer(&types, Some("org.keepassxc.KeePassXC")),
            Some(Sensitivity::Secret)
        );
    }

    #[test]
    fn copies_from_excluded_apps_are_concealed_whatever_they_hold() {
        let types = ["text/plain".to_string()];
        let c = classifier();
        for app in [
            "org.keepassxc.KeePassXC",
            "Bitwarden",
            "1password",
            "proton-pass",
        ] {
            assert_eq!(
                c.offer(&types, Some(app)),
                Some(Sensitivity::Concealed),
                "{app}"
            );
        }
        assert_eq!(c.offer(&types, Some("firefox")), None);
        assert_eq!(c.offer(&types, None), None);
    }
}
