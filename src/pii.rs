// Internal PII scanner for the eval_prompt guard.
//
// This module is NEVER exposed to user code. It is an internal runtime guard
// that detects potential PII patterns in prompt inputs. Simple regex-style
// matching — intentionally conservative (may have false positives).
//
// Not a builtin. Not stdlib. Not callable from .ag code.

/// Types of PII patterns detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PiiType {
    Email,
    Phone,
    CreditCard,
    CzechBirthNumber,
    Iban,
    Ipv4,
    Ssn,
}

impl std::fmt::Display for PiiType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PiiType::Email => write!(f, "email"),
            PiiType::Phone => write!(f, "phone"),
            PiiType::CreditCard => write!(f, "credit_card"),
            PiiType::CzechBirthNumber => write!(f, "czech_birth_number"),
            PiiType::Iban => write!(f, "iban"),
            PiiType::Ipv4 => write!(f, "ipv4"),
            PiiType::Ssn => write!(f, "ssn"),
        }
    }
}

/// Result of a PII scan.
#[derive(Debug, Clone)]
pub struct PiiScanResult {
    pub detected: Vec<PiiType>,
}

impl PiiScanResult {
    pub fn is_clean(&self) -> bool {
        self.detected.is_empty()
    }

    pub fn types_str(&self) -> String {
        let names: Vec<&str> = self
            .detected
            .iter()
            .map(|t| match t {
                PiiType::Email => "email",
                PiiType::Phone => "phone",
                PiiType::CreditCard => "credit_card",
                PiiType::CzechBirthNumber => "czech_birth_number",
                PiiType::Iban => "iban",
                PiiType::Ipv4 => "ipv4",
                PiiType::Ssn => "ssn",
            })
            .collect();
        names.join(", ")
    }
}

/// Scan text for potential PII patterns.
/// Returns which types were detected (deduplicated).
pub fn scan(text: &str) -> PiiScanResult {
    let mut found = Vec::new();

    if has_email(text) {
        found.push(PiiType::Email);
    }
    if has_phone(text) {
        found.push(PiiType::Phone);
    }
    if has_credit_card(text) {
        found.push(PiiType::CreditCard);
    }
    if has_czech_birth_number(text) {
        found.push(PiiType::CzechBirthNumber);
    }
    if has_iban(text) {
        found.push(PiiType::Iban);
    }
    if has_ipv4(text) {
        found.push(PiiType::Ipv4);
    }
    if has_ssn(text) {
        found.push(PiiType::Ssn);
    }

    PiiScanResult { detected: found }
}

// --- Pattern matchers ---
// Hand-rolled matching (no regex crate). Simple but effective for common patterns.

fn has_email(text: &str) -> bool {
    // Look for word@word.word pattern
    for (i, ch) in text.char_indices() {
        if ch == '@' && i > 0 {
            // Check left side: at least one alnum/dot/+/-/_
            let left_ok = text[..i]
                .chars()
                .rev()
                .take_while(|c| {
                    c.is_ascii_alphanumeric() || *c == '.' || *c == '+' || *c == '-' || *c == '_'
                })
                .count()
                > 0;
            // Check right side: at least one dot after some chars
            let right = &text[i + 1..];
            let domain_part: String = right
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            let right_ok = domain_part.contains('.')
                && domain_part.len() >= 3
                && !domain_part.starts_with('.')
                && !domain_part.ends_with('.');
            if left_ok && right_ok {
                return true;
            }
        }
    }
    false
}

fn has_phone(text: &str) -> bool {
    // Look for sequences of 8+ digits (possibly with spaces, dashes, dots)
    // optionally starting with +
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        // Optional +
        if chars[i] == '+' {
            i += 1;
            if i >= chars.len() || !chars[i].is_ascii_digit() {
                continue;
            }
        }
        if i < chars.len() && chars[i].is_ascii_digit() {
            let mut digit_count = 0;
            let mut total_len = 0;
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || chars[i] == ' '
                    || chars[i] == '-'
                    || chars[i] == '.')
            {
                if chars[i].is_ascii_digit() {
                    digit_count += 1;
                }
                total_len += 1;
                i += 1;
                // Don't let it run forever on huge non-phone sequences
                if total_len > 20 {
                    break;
                }
            }
            // Phone: 8-15 digits
            if digit_count >= 8 && digit_count <= 15 {
                // Check it's not part of a hash or UUID (no hex letters adjacent)
                let before_ok = start == 0 || !chars[start - 1].is_ascii_alphanumeric();
                let after_ok = i >= chars.len() || !chars[i].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

fn has_credit_card(text: &str) -> bool {
    // 4 groups of 4 digits, separated by spaces or dashes (or no separator)
    let digits_and_seps: Vec<(usize, char)> = text
        .char_indices()
        .filter(|(_, c)| c.is_ascii_digit() || *c == ' ' || *c == '-')
        .collect();

    // Scan for 13-19 digit sequences
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut digits = Vec::new();
            while i < chars.len()
                && (chars[i].is_ascii_digit() || chars[i] == ' ' || chars[i] == '-')
            {
                if chars[i].is_ascii_digit() {
                    digits.push(chars[i]);
                }
                i += 1;
                if digits.len() > 19 {
                    break;
                }
            }
            if digits.len() >= 13 && digits.len() <= 19 {
                // Check word boundaries
                let before_ok = start == 0 || !chars[start - 1].is_ascii_alphanumeric();
                let after_ok = i >= chars.len() || !chars[i].is_ascii_alphanumeric();
                if before_ok && after_ok && luhn_check(&digits) {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Luhn algorithm for credit card validation.
fn luhn_check(digits: &[char]) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for &ch in digits.iter().rev() {
        let mut d = ch as u32 - '0' as u32;
        if double {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        double = !double;
    }
    sum % 10 == 0
}

fn has_czech_birth_number(text: &str) -> bool {
    // Format: YYMMDD/NNNN or YYMMDDNNNN (with optional slash)
    // Month can be 01-12 or 51-62 (female), day 01-31
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 9 <= len {
        // Check if we have 6 digits, optional slash, 3-4 digits
        if bytes[i].is_ascii_digit() {
            let d = |pos: usize| -> Option<u8> {
                if pos < len && bytes[pos].is_ascii_digit() {
                    Some(bytes[pos] - b'0')
                } else {
                    None
                }
            };

            // Parse YYMMDD
            if let (Some(y1), Some(y2), Some(m1), Some(m2), Some(d1), Some(d2)) =
                (d(i), d(i + 1), d(i + 2), d(i + 3), d(i + 4), d(i + 5))
            {
                let month = m1 * 10 + m2;
                let day = d1 * 10 + d2;
                // Month: 01-12 or 51-62 (female)
                let month_ok = (month >= 1 && month <= 12) || (month >= 51 && month <= 62);
                let day_ok = day >= 1 && day <= 31;

                if month_ok && day_ok {
                    let after6 = i + 6;
                    // Check for slash variant: NNNNNN/NNN(N)
                    if after6 < len && bytes[after6] == b'/' {
                        let suffix_start = after6 + 1;
                        let suffix_digits = (suffix_start..len)
                            .take_while(|&j| bytes[j].is_ascii_digit())
                            .count();
                        if (suffix_digits == 3 || suffix_digits == 4)
                            && is_word_boundary(bytes, i, suffix_start + suffix_digits, len)
                        {
                            return true;
                        }
                    }
                    // No-slash variant: NNNNNNNNNN (10 digits)
                    let suffix_digits = (after6..len)
                        .take_while(|&j| bytes[j].is_ascii_digit())
                        .count();
                    if (suffix_digits == 3 || suffix_digits == 4)
                        && is_word_boundary(bytes, i, after6 + suffix_digits, len)
                    {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

fn has_iban(text: &str) -> bool {
    // Two uppercase letters + 2 digits + 8-30 alphanumeric (with optional spaces)
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 5 <= len {
        if bytes[i].is_ascii_uppercase()
            && i + 1 < len
            && bytes[i + 1].is_ascii_uppercase()
            && i + 2 < len
            && bytes[i + 2].is_ascii_digit()
            && i + 3 < len
            && bytes[i + 3].is_ascii_digit()
        {
            // Check word boundary before
            if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
                i += 1;
                continue;
            }
            // Count remaining alnum chars (skip spaces)
            let mut j = i + 4;
            let mut alnum_count = 0;
            while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b' ') {
                if bytes[j].is_ascii_alphanumeric() {
                    alnum_count += 1;
                }
                j += 1;
                if alnum_count > 30 {
                    break;
                }
            }
            if alnum_count >= 8 && alnum_count <= 30 {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn has_ipv4(text: &str) -> bool {
    // N.N.N.N where each N is 0-255
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i].is_ascii_digit() {
            // Try to parse 4 octets
            let start = i;
            let mut octets = Vec::new();
            let mut j = i;
            loop {
                // Parse number
                let num_start = j;
                while j < len && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j == num_start {
                    break;
                }
                let num_str: String = text[num_start..j].to_string();
                if let Ok(n) = num_str.parse::<u32>() {
                    if n > 255 {
                        break;
                    }
                    octets.push(n);
                } else {
                    break;
                }
                if octets.len() == 4 {
                    break;
                }
                // Expect dot
                if j < len && bytes[j] == b'.' {
                    j += 1;
                } else {
                    break;
                }
            }
            if octets.len() == 4 {
                // Skip common non-PII IPs: 0.0.0.0, 127.x.x.x, 255.255.255.255
                let is_trivial =
                    (octets[0] == 0 && octets[1] == 0 && octets[2] == 0 && octets[3] == 0)
                        || octets[0] == 127
                        || (octets[0] == 255
                            && octets[1] == 255
                            && octets[2] == 255
                            && octets[3] == 255);
                if !is_trivial && is_word_boundary(bytes, start, j, len) {
                    return true;
                }
            }
            i = if j > i { j } else { i + 1 };
        } else {
            i += 1;
        }
    }
    false
}

fn has_ssn(text: &str) -> bool {
    // US SSN: NNN-NN-NNNN
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 10 < len {
        if bytes[i].is_ascii_digit()
            && i + 1 < len
            && bytes[i + 1].is_ascii_digit()
            && i + 2 < len
            && bytes[i + 2].is_ascii_digit()
            && i + 3 < len
            && bytes[i + 3] == b'-'
            && i + 4 < len
            && bytes[i + 4].is_ascii_digit()
            && i + 5 < len
            && bytes[i + 5].is_ascii_digit()
            && i + 6 < len
            && bytes[i + 6] == b'-'
            && i + 7 < len
            && bytes[i + 7].is_ascii_digit()
            && i + 8 < len
            && bytes[i + 8].is_ascii_digit()
            && i + 9 < len
            && bytes[i + 9].is_ascii_digit()
            && i + 10 < len
            && bytes[i + 10].is_ascii_digit()
        {
            if is_word_boundary(bytes, i, i + 11, len) {
                // Reject known invalid: 000-xx-xxxx, xxx-00-xxxx, xxx-xx-0000
                let area =
                    (bytes[i] - b'0') * 100 + (bytes[i + 1] - b'0') * 10 + (bytes[i + 2] - b'0');
                let group = (bytes[i + 4] - b'0') * 10 + (bytes[i + 5] - b'0');
                let serial = (bytes[i + 7] - b'0') as u32 * 1000
                    + (bytes[i + 8] - b'0') as u32 * 100
                    + (bytes[i + 9] - b'0') as u32 * 10
                    + (bytes[i + 10] - b'0') as u32;
                if area != 0 && group != 0 && serial != 0 {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn is_word_boundary(bytes: &[u8], start: usize, end: usize, len: usize) -> bool {
    let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
    let after_ok = end >= len || !bytes[end].is_ascii_alphanumeric();
    before_ok && after_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Email ---

    #[test]
    fn detect_email() {
        let r = scan("Contact us at user@example.com for details");
        assert!(r.detected.contains(&PiiType::Email));
    }

    #[test]
    fn detect_email_with_plus() {
        let r = scan("Send to user+tag@domain.co.uk");
        assert!(r.detected.contains(&PiiType::Email));
    }

    #[test]
    fn no_email_in_clean_text() {
        let r = scan("Hello world, this is a test");
        assert!(!r.detected.contains(&PiiType::Email));
    }

    #[test]
    fn no_email_at_sign_alone() {
        let r = scan("@ or @@ or a@b");
        assert!(!r.detected.contains(&PiiType::Email));
    }

    // --- Phone ---

    #[test]
    fn detect_phone_intl() {
        let r = scan("Call me at +420 123 456 789");
        assert!(r.detected.contains(&PiiType::Phone));
    }

    #[test]
    fn detect_phone_dashes() {
        let r = scan("Phone: 123-456-7890");
        assert!(r.detected.contains(&PiiType::Phone));
    }

    #[test]
    fn no_phone_short_number() {
        let r = scan("Code: 12345");
        assert!(!r.detected.contains(&PiiType::Phone));
    }

    // --- Credit Card ---

    #[test]
    fn detect_credit_card_spaces() {
        let r = scan("Card: 4111 1111 1111 1111");
        assert!(r.detected.contains(&PiiType::CreditCard));
    }

    #[test]
    fn detect_credit_card_dashes() {
        let r = scan("Card: 4111-1111-1111-1111");
        assert!(r.detected.contains(&PiiType::CreditCard));
    }

    #[test]
    fn no_credit_card_random_digits() {
        let r = scan("Number: 1234567890123456");
        // Random digits won't pass Luhn
        assert!(!r.detected.contains(&PiiType::CreditCard));
    }

    // --- Czech Birth Number ---

    #[test]
    fn detect_birth_number_slash() {
        let r = scan("RČ: 900101/1234");
        assert!(r.detected.contains(&PiiType::CzechBirthNumber));
    }

    #[test]
    fn detect_birth_number_female() {
        let r = scan("RČ: 905201/1234");
        assert!(r.detected.contains(&PiiType::CzechBirthNumber));
    }

    #[test]
    fn detect_birth_number_no_slash() {
        let r = scan("ID: 9001011234");
        assert!(r.detected.contains(&PiiType::CzechBirthNumber));
    }

    #[test]
    fn no_birth_number_bad_month() {
        let r = scan("Num: 901301/1234");
        assert!(!r.detected.contains(&PiiType::CzechBirthNumber));
    }

    // --- IBAN ---

    #[test]
    fn detect_iban() {
        let r = scan("Account: CZ65 0800 0000 1920 0014 5399");
        assert!(r.detected.contains(&PiiType::Iban));
    }

    #[test]
    fn detect_iban_no_spaces() {
        let r = scan("IBAN: DE89370400440532013000");
        assert!(r.detected.contains(&PiiType::Iban));
    }

    #[test]
    fn no_iban_too_short() {
        let r = scan("Code: CZ12 3456");
        assert!(!r.detected.contains(&PiiType::Iban));
    }

    // --- IPv4 ---

    #[test]
    fn detect_ipv4() {
        let r = scan("Server at 192.168.1.100");
        assert!(r.detected.contains(&PiiType::Ipv4));
    }

    #[test]
    fn no_ipv4_localhost() {
        let r = scan("Connect to 127.0.0.1");
        assert!(!r.detected.contains(&PiiType::Ipv4));
    }

    #[test]
    fn no_ipv4_zeros() {
        let r = scan("Default: 0.0.0.0");
        assert!(!r.detected.contains(&PiiType::Ipv4));
    }

    #[test]
    fn no_ipv4_out_of_range() {
        let r = scan("Not IP: 999.999.999.999");
        assert!(!r.detected.contains(&PiiType::Ipv4));
    }

    // --- SSN ---

    #[test]
    fn detect_ssn() {
        let r = scan("SSN: 123-45-6789");
        assert!(r.detected.contains(&PiiType::Ssn));
    }

    #[test]
    fn no_ssn_invalid_zeros() {
        let r = scan("Num: 000-12-3456");
        assert!(!r.detected.contains(&PiiType::Ssn));
    }

    // --- Combined ---

    #[test]
    fn clean_text_no_pii() {
        let r = scan("The quick brown fox jumps over the lazy dog. Score: 42.");
        assert!(r.is_clean());
    }

    #[test]
    fn multiple_pii_types() {
        let r = scan("Email: test@example.com, Phone: +1 555 123 4567, IP: 10.0.0.1");
        assert!(r.detected.contains(&PiiType::Email));
        assert!(r.detected.contains(&PiiType::Phone));
        assert!(r.detected.contains(&PiiType::Ipv4));
    }

    #[test]
    fn types_str_format() {
        let r = scan("Contact: user@test.com and call +420 111 222 333");
        let s = r.types_str();
        assert!(s.contains("email"));
        assert!(s.contains("phone"));
    }

    #[test]
    fn empty_string_is_clean() {
        assert!(scan("").is_clean());
    }
}
