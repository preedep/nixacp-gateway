use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol_schema::{NewSessionResponse, SessionId};
use dashmap::DashMap;
use domain::entities::message::Message;
use uuid::Uuid;

const SESSION_TTL: Duration = Duration::from_secs(30 * 60);

struct SessionEntry {
    history: Vec<Message>,
    last_access: Instant,
    cwd: std::path::PathBuf,
}

/// In-memory ACP session store backed by DashMap with 30-minute TTL.
pub struct AcpSessionStore {
    sessions: Arc<DashMap<SessionId, SessionEntry>>,
}

impl AcpSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }

    /// Create a new session and return its ID plus the SDK `NewSessionResponse`.
    pub fn create_session(&self, cwd: std::path::PathBuf) -> (SessionId, NewSessionResponse) {
        let id = SessionId::new(Uuid::new_v4().to_string());
        self.sessions.insert(
            id.clone(),
            SessionEntry {
                history: Vec::new(),
                last_access: Instant::now(),
                cwd,
            },
        );
        let response = NewSessionResponse::new(id.clone());
        (id, response)
    }

    /// Append messages to a session's history.
    ///
    /// Returns `false` if the session does not exist.
    pub fn push_messages(&self, session_id: &SessionId, messages: Vec<Message>) -> bool {
        match self.sessions.get_mut(session_id) {
            Some(mut entry) => {
                entry.last_access = Instant::now();
                entry.history.extend(messages);
                true
            }
            None => false,
        }
    }

    /// Snapshot the message history and working directory of a session.
    pub fn snapshot(&self, session_id: &SessionId) -> Option<(Vec<Message>, std::path::PathBuf)> {
        self.sessions.get_mut(session_id).map(|mut entry| {
            entry.last_access = Instant::now();
            (entry.history.clone(), entry.cwd.clone())
        })
    }

    /// Evict sessions that have been idle longer than the TTL.
    pub fn evict_expired(&self) {
        self.sessions
            .retain(|_, entry| entry.last_access.elapsed() < SESSION_TTL);
    }

    /// Remove a session by ID.
    pub fn remove_session(&self, session_id: &SessionId) {
        self.sessions.remove(session_id);
    }
}

impl Default for AcpSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::entities::message::Message;

    #[test]
    fn create_and_lookup_session() {
        let store = AcpSessionStore::new();
        let (id, _resp) = store.create_session("/tmp".into());
        let snapshot = store.snapshot(&id);
        assert!(snapshot.is_some());
        let (history, cwd) = snapshot.unwrap();
        assert!(history.is_empty());
        assert_eq!(cwd, std::path::PathBuf::from("/tmp"));
    }

    #[test]
    fn push_messages_to_session() {
        let store = AcpSessionStore::new();
        let (id, _) = store.create_session("/tmp".into());
        assert!(store.push_messages(&id, vec![Message::user("hello")]));
        let (history, _) = store.snapshot(&id).unwrap();
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn push_messages_unknown_session_returns_false() {
        let store = AcpSessionStore::new();
        let unknown = SessionId::new("does-not-exist");
        assert!(!store.push_messages(&unknown, vec![Message::user("hi")]));
    }

    #[test]
    fn remove_session() {
        let store = AcpSessionStore::new();
        let (id, _) = store.create_session("/tmp".into());
        store.remove_session(&id);
        assert!(store.snapshot(&id).is_none());
    }

    #[test]
    fn evict_expired_leaves_fresh_sessions() {
        let store = AcpSessionStore::new();
        let (id, _) = store.create_session("/tmp".into());
        store.evict_expired();
        assert!(store.snapshot(&id).is_some());
    }
}
