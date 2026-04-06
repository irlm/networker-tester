use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

pub struct LogBuffer {
    entries: Mutex<VecDeque<LogEntry>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        })
    }

    pub fn push(&self, entry: LogEntry) {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= self.capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    pub fn recent(
        &self,
        count: usize,
        level_filter: Option<&str>,
        search: Option<&str>,
    ) -> Vec<LogEntry> {
        let entries = self.entries.lock().unwrap();
        entries
            .iter()
            .rev()
            .filter(|e| {
                if let Some(level) = level_filter {
                    if !e.level.eq_ignore_ascii_case(level) {
                        return false;
                    }
                }
                if let Some(q) = search {
                    let q_lower = q.to_lowercase();
                    if !e.message.to_lowercase().contains(&q_lower)
                        && !e.target.to_lowercase().contains(&q_lower)
                    {
                        return false;
                    }
                }
                true
            })
            .take(count)
            .cloned()
            .collect()
    }
}

/// A field visitor that extracts the message from tracing events.
struct MessageVisitor {
    message: String,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
        }
    }
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else if self.message.is_empty() {
            // Fallback: use the first non-message field as the log message
            self.message = format!("{}={:?}", field.name(), value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }
}

pub struct LogBufferLayer {
    buffer: Arc<LogBuffer>,
}

impl LogBufferLayer {
    pub fn new(buffer: Arc<LogBuffer>) -> Self {
        Self { buffer }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogBufferLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        let entry = LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message: visitor.message,
        };
        self.buffer.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_buffer_capacity() {
        let buf = LogBuffer::new(3);
        for i in 0..5 {
            buf.push(LogEntry {
                timestamp: format!("t{i}"),
                level: "INFO".into(),
                target: "test".into(),
                message: format!("msg{i}"),
            });
        }
        let recent = buf.recent(10, None, None);
        assert_eq!(recent.len(), 3);
        // Most recent first
        assert_eq!(recent[0].message, "msg4");
        assert_eq!(recent[2].message, "msg2");
    }

    #[test]
    fn log_buffer_filter_level() {
        let buf = LogBuffer::new(10);
        buf.push(LogEntry {
            timestamp: "t0".into(),
            level: "INFO".into(),
            target: "test".into(),
            message: "info msg".into(),
        });
        buf.push(LogEntry {
            timestamp: "t1".into(),
            level: "ERROR".into(),
            target: "test".into(),
            message: "error msg".into(),
        });
        let recent = buf.recent(10, Some("error"), None);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].level, "ERROR");
    }

    #[test]
    fn log_buffer_filter_search() {
        let buf = LogBuffer::new(10);
        buf.push(LogEntry {
            timestamp: "t0".into(),
            level: "INFO".into(),
            target: "auth".into(),
            message: "login success".into(),
        });
        buf.push(LogEntry {
            timestamp: "t1".into(),
            level: "INFO".into(),
            target: "db".into(),
            message: "migration complete".into(),
        });
        let recent = buf.recent(10, None, Some("login"));
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].message, "login success");
    }
}
