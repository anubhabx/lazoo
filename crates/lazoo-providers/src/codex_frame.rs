//! Bounded NDJSON framing. A large viewer response is drained without retaining its body;
//! top-level correlation survives regardless of the provider's field order.
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub(super) const MAX_LINE_BYTES: usize = 512 * 1024;

pub(super) enum BrokerLine {
    Message(Vec<u8>),
    Oversized {
        id: Option<String>,
        has_method: bool,
    },
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // Independent lexical flags keep the bounded JSON envelope scanner explicit.
struct EnvelopeProbe {
    depth: usize,
    in_string: bool,
    escaped: bool,
    capture: bool,
    value: bool,
    token: Vec<u8>,
    key: Option<String>,
    id: Option<String>,
    has_method: bool,
}

impl EnvelopeProbe {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.in_string {
                if self.capture && self.token.len() < 256 {
                    self.token.push(byte);
                }
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                    if self.capture {
                        let token = serde_json::from_slice::<String>(&self.token).ok();
                        if self.value {
                            if self.key.as_deref() == Some("id") {
                                self.id = token;
                            }
                        } else {
                            self.has_method |= token.as_deref() == Some("method");
                            self.key = token;
                        }
                    }
                }
                continue;
            }
            match byte {
                b'"' => {
                    self.in_string = true;
                    self.capture =
                        self.depth == 1 && (!self.value || self.key.as_deref() == Some("id"));
                    self.token.clear();
                    if self.capture {
                        self.token.push(byte);
                    }
                }
                b'{' | b'[' => self.depth = self.depth.saturating_add(1),
                b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                b':' if self.depth == 1 => self.value = true,
                b',' if self.depth == 1 => {
                    self.value = false;
                    self.key = None;
                }
                _ => {}
            }
        }
    }
}

pub(super) async fn read_broker_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<BrokerLine>> {
    let mut line = Vec::new();
    let mut probe = None;
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return Ok(match probe {
                Some(EnvelopeProbe { id, has_method, .. }) => {
                    Some(BrokerLine::Oversized { id, has_method })
                }
                None if line.is_empty() => None,
                None => Some(BrokerLine::Message(line)),
            });
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |position| position + 1);
        let content = newline.map_or(buffer, |position| &buffer[..position]);
        if probe.is_none() && line.len().saturating_add(content.len()) > MAX_LINE_BYTES {
            let mut envelope = EnvelopeProbe::default();
            envelope.feed(&line);
            line.clear();
            probe = Some(envelope);
        }
        if let Some(probe) = &mut probe {
            probe.feed(content);
        } else {
            line.extend_from_slice(content);
        }
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(match probe {
                Some(EnvelopeProbe { id, has_method, .. }) => {
                    BrokerLine::Oversized { id, has_method }
                }
                None => BrokerLine::Message(line),
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oversized_envelopes_preserve_top_level_correlation_and_next_frame() {
        let body = serde_json::json!({"nested": [{"id":"spoof","method":"spoof"}], "text":"\\\"界".repeat(MAX_LINE_BYTES)}).to_string();
        for frame in [
            format!(r#"{{"id":"view:7","result":{body}}}"#),
            format!(r#"{{"result":{body},"id":"view:7"}}"#),
        ] {
            let bytes = format!("{frame}\r\n{{\"method\":\"turn/completed\"}}\n");
            let mut reader = tokio::io::BufReader::with_capacity(7, bytes.as_bytes());
            assert!(
                matches!(read_broker_line(&mut reader).await.unwrap(), Some(BrokerLine::Oversized { id: Some(id), has_method: false }) if id == "view:7")
            );
            assert!(matches!(
                read_broker_line(&mut reader).await.unwrap(),
                Some(BrokerLine::Message(_))
            ));
        }
        let bytes = format!(r#"{{"id":"view:7","params":{body},"method":"item/tool/call"}}"#);
        assert!(matches!(
            read_broker_line(&mut bytes.as_bytes()).await.unwrap(),
            Some(BrokerLine::Oversized {
                has_method: true,
                ..
            })
        ));
    }
}
