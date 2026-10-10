use super::*;

pub(super) const MAX_LINE_BYTES: usize = 64 * 1024;

/// Bounded rolling line buffer. Scan both completed lines and the live
/// partial line because interactive status output often omits newlines.
#[derive(Default)]
pub(super) struct OutputScanner {
    pub(super) line: Vec<u8>,
}

impl OutputScanner {
    pub(super) fn feed(&mut self, bytes: &[u8], patterns: &[CompiledPattern]) -> Option<RlEvent> {
        let mut found = None;
        for &byte in bytes {
            if matches!(byte, b'\r' | b'\n') {
                if let Some(event) = scan_line(&self.line, patterns) {
                    if event.resets_at.is_some()
                        || found
                            .as_ref()
                            .is_none_or(|previous: &RlEvent| previous.resets_at.is_none())
                    {
                        found = Some(event);
                    }
                }
                self.line.clear();
            } else {
                if self.line.len() == MAX_LINE_BYTES {
                    self.line.drain(..MAX_LINE_BYTES / 2);
                }
                self.line.push(byte);
            }
        }
        if let Some(event) = scan_line(&self.line, patterns) {
            if event.resets_at.is_some()
                || found
                    .as_ref()
                    .is_none_or(|previous| previous.resets_at.is_none())
            {
                found = Some(event);
            }
        }
        found
    }

    pub(super) fn finish(&self, patterns: &[CompiledPattern]) -> Option<RlEvent> {
        scan_line(&self.line, patterns)
    }
}

pub(super) fn scan_line(buf: &[u8], patterns: &[CompiledPattern]) -> Option<RlEvent> {
    if buf.is_empty() {
        return None;
    }
    // Strip basic ANSI CSI/OSC sequences before matching so escape codes
    // baked into the runtime's status line don't defeat the regex.
    let stripped = strip_ansi(buf);
    let line = String::from_utf8_lossy(&stripped).into_owned();
    for p in patterns {
        if p.matcher.is_match(&line) {
            let resets_at = parse_resets(&line, p);
            return Some(RlEvent {
                raw: line,
                resets_at,
            });
        }
    }
    None
}

pub(super) fn strip_ansi(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == 0x1b && i + 1 < input.len() {
            match input[i + 1] {
                b'[' => {
                    // CSI: skip until a byte in 0x40..=0x7E
                    i += 2;
                    while i < input.len() && !(0x40..=0x7e).contains(&input[i]) {
                        i += 1;
                    }
                    if i < input.len() {
                        i += 1;
                    }
                }
                b']' => {
                    // OSC: skip until BEL (0x07) or ST (ESC \)
                    i += 2;
                    while i < input.len() {
                        if input[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if input[i] == 0x1b && i + 1 < input.len() && input[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Other ESC X — skip the two bytes
                    i += 2;
                }
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

#[derive(Clone)]
pub(super) struct CompiledPattern {
    pub(super) matcher: Regex,
    resets_at: Option<Regex>,
    resets_in: Option<Regex>,
}

pub(super) fn compile_patterns(src: &[RateLimitPattern]) -> Result<Vec<CompiledPattern>> {
    src.iter()
        .map(|p| {
            Ok(CompiledPattern {
                matcher: Regex::new(&p.r#match)
                    .with_context(|| format!("compile match regex `{}`", p.r#match))?,
                resets_at: p
                    .resets_at_capture
                    .as_deref()
                    .map(Regex::new)
                    .transpose()
                    .context("compile resets_at_capture")?,
                resets_in: p
                    .resets_in_capture
                    .as_deref()
                    .map(Regex::new)
                    .transpose()
                    .context("compile resets_in_capture")?,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(super) struct RlEvent {
    pub(super) raw: String,
    pub(super) resets_at: Option<f64>,
}

pub(super) fn parse_resets(line: &str, p: &CompiledPattern) -> Option<f64> {
    if let Some(re) = &p.resets_at {
        if let Some(cap) = re.captures(line) {
            if let Some(m) = cap.get(1) {
                if let Some(timestamp) = parse_clock_time(m.as_str()) {
                    return Some(timestamp);
                }
            }
        }
    }
    if let Some(re) = &p.resets_in {
        if let Some(cap) = re.captures(line) {
            if let Some(m) = cap.get(1) {
                if let Some(secs) = parse_duration(m.as_str()) {
                    return Some(now() + secs as f64);
                }
            }
        }
    }
    None
}

/// Parse "4pm", "16:00", "16:00 UTC", "4:30 pm" → next future occurrence (UNIX seconds).
pub(super) fn parse_clock_time(s: &str) -> Option<f64> {
    parse_clock_time_at(s, Utc::now())
}

pub(super) fn parse_clock_time_at(s: &str, now: chrono::DateTime<Utc>) -> Option<f64> {
    let s = s.trim();
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(date.timestamp() as f64);
    }
    let normalized = s.to_lowercase();
    let (clock, zone) = normalized
        .rsplit_once(char::is_whitespace)
        .filter(|(_, zone)| !matches!(*zone, "am" | "pm"))
        .map(|(clock, zone)| (clock, Some(zone)))
        .unwrap_or((&normalized, None));
    let offset = match zone {
        Some("utc" | "gmt" | "z") => Some(FixedOffset::east_opt(0)?),
        Some(zone) if zone.starts_with(['+', '-']) => {
            let digits = zone[1..].replace(':', "");
            if digits.len() != 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let hours = digits[..2].parse::<i32>().ok()?;
            let minutes = digits[2..].parse::<i32>().ok()?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = (hours * 60 + minutes) * 60;
            Some(FixedOffset::east_opt(if zone.starts_with('-') {
                -seconds
            } else {
                seconds
            })?)
        }
        Some(_) => return None, // ambiguous abbreviations must not become host-local time
        None => None,
    };
    let formats = ["%I%P", "%I%p", "%I:%M%P", "%I:%M%p", "%H:%M"];
    let mut candidate = clock.replace(' ', "");
    if !candidate.contains(':') {
        let suffix = if candidate.ends_with("am") || candidate.ends_with("pm") {
            candidate.len() - 2
        } else {
            candidate.len()
        };
        if candidate[..suffix].bytes().all(|c| c.is_ascii_digit()) {
            candidate.insert_str(suffix, ":00");
        }
    }
    for f in formats {
        if let Ok(t) = NaiveTime::parse_from_str(&candidate, f) {
            if let Some(offset) = offset {
                let zoned_now = now.with_timezone(&offset);
                let mut date = zoned_now.date_naive().and_time(t);
                if date <= zoned_now.naive_local() {
                    date += chrono::Duration::days(1);
                }
                return offset
                    .from_local_datetime(&date)
                    .single()
                    .map(|d| d.timestamp() as f64);
            }
            let local_now = now.with_timezone(&Local);
            let mut d = local_now
                .date_naive()
                .and_hms_opt(t.hour(), t.minute(), 0)?;
            if d <= local_now.naive_local() {
                d += chrono::Duration::days(1);
            }
            let local = Local.from_local_datetime(&d).earliest()?;
            return Some(local.with_timezone(&Utc).timestamp() as f64);
        }
    }
    None
}

/// Parse "5h", "5h 15m", "30m", "120s", "2 hours" → seconds.
pub(super) fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    let mut total: u64 = 0;
    let mut buf = String::new();
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if c.is_ascii_digit() {
            buf.push(c);
            continue;
        }
        if buf.is_empty() {
            continue;
        }
        let n: u64 = buf.parse().ok()?;
        buf.clear();
        // Read the unit greedily.
        let mut unit = String::from(c);
        while let Some(&p) = iter.peek() {
            if p.is_ascii_alphabetic() {
                unit.push(p);
                iter.next();
            } else {
                break;
            }
        }
        let unit = unit.trim();
        let mul = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            _ => return None,
        };
        total = total.checked_add(n.checked_mul(mul)?)?;
    }
    if !buf.is_empty() {
        // bare number: treat as seconds
        total = total.checked_add(buf.parse::<u64>().ok()?)?;
    }
    if total == 0 {
        None
    } else {
        Some(total)
    }
}
