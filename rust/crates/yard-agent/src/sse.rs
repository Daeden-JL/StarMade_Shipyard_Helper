//! Server-sent events: cutting a byte stream that arrives in arbitrary
//! pieces into events.

/// One event: its `event:` name, if it had one, and its `data:` lines
/// joined by newlines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Collects bytes until whole events can be handed out. A piece may end
/// anywhere: inside a line, inside a character, between the two bytes of a
/// CRLF.
#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the next bytes and returns the events they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&String::from_utf8_lossy(&line), &mut out);
        }
        out
    }

    /// The event left unfinished when the stream ended without a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let rest = std::mem::take(&mut self.buffer);
        let mut out = Vec::new();
        if !rest.is_empty() {
            self.line(String::from_utf8_lossy(&rest).trim_end_matches('\r'), &mut out);
        }
        self.line("", &mut out);
        out.pop()
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if self.event.is_some() || !self.data.is_empty() {
                out.push(SseEvent { event: self.event.take(), data: std::mem::take(&mut self.data).join("\n") });
            }
            return;
        }
        // A line starting with a colon is a comment, used as a keep-alive.
        let (field, value) = match line.split_once(':') {
            Some(("", _)) => return,
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: Option<&str>, data: &str) -> SseEvent {
        SseEvent { event: name.map(str::to_string), data: data.to_string() }
    }

    #[test]
    fn events_come_out_whole_however_the_bytes_are_cut() {
        let stream = "event: message_start\r\ndata: {\"a\":\"gr\u{fc}n\"}\r\n\r\n: keep-alive\n\ndata: one\ndata: two\n\nevent: ping\ndata:\n\n";
        let expected = vec![event(Some("message_start"), "{\"a\":\"gr\u{fc}n\"}"), event(None, "one\ntwo"), event(Some("ping"), "")];
        let bytes = stream.as_bytes();
        for size in 1..bytes.len() {
            let mut decoder = SseDecoder::new();
            let got: Vec<SseEvent> = bytes.chunks(size).flat_map(|chunk| decoder.push(chunk)).collect();
            assert_eq!(got, expected, "cut into pieces of {size} bytes");
            assert_eq!(decoder.finish(), None);
        }
    }

    #[test]
    fn a_last_event_without_its_blank_line_is_still_handed_out() {
        let mut decoder = SseDecoder::new();
        assert_eq!(decoder.push(b"data: [DONE]"), vec![]);
        assert_eq!(decoder.finish(), Some(event(None, "[DONE]")));
    }
}
