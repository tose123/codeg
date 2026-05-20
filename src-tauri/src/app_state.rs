use std::path::PathBuf;
use std::sync::Arc;

use crate::acp::manager::ConnectionManager;
use crate::acp::InternalEventBus;
use crate::chat_channel::manager::ChatChannelManager;
use crate::db::AppDatabase;
use crate::pet_state_mapper::PetStateHandle;
use crate::terminal::manager::TerminalManager;
use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};
use crate::web::WebServerState;
use crate::workspace_transfer::WorkspaceTransferManager;

pub struct AppState {
    pub db: AppDatabase,
    pub connection_manager: ConnectionManager,
    pub terminal_manager: TerminalManager,
    pub event_broadcaster: Arc<WebEventBroadcaster>,
    /// Process-wide bus for typed `Arc<EventEnvelope>` delivery to
    /// in-process consumers (lifecycle, pet state mapper, chat-channel
    /// subscribers). Distinct from `event_broadcaster`, which carries
    /// JSON-shaped `WebEvent`s for transport-bound delivery.
    pub acp_event_bus: Arc<InternalEventBus>,
    pub emitter: EventEmitter,
    pub data_dir: PathBuf,
    pub web_server_state: WebServerState,
    pub chat_channel_manager: ChatChannelManager,
    pub workspace_transfer: Arc<WorkspaceTransferManager>,
    /// Latest ambient `PetState` written by `pet_state_subscriber_task`.
    /// Read by `pet_get_current_state` so a freshly-opened pet window can
    /// pick up the current state without waiting for the next transition.
    pub pet_state: PetStateHandle,
}

pub fn default_connection_manager() -> ConnectionManager {
    ConnectionManager::new()
}

pub fn default_terminal_manager() -> TerminalManager {
    TerminalManager::new()
}

pub fn default_chat_channel_manager() -> ChatChannelManager {
    ChatChannelManager::new()
}

impl AppState {
    /// Test-only constructor: build an `AppState` wired to an in-memory
    /// database and a `WebOnly` event emitter. Suitable for axum-test driven
    /// HTTP integration tests where no Tauri runtime is available.
    ///
    /// `data_dir` is a temp directory; handlers that touch it must use
    /// `tempfile::tempdir()` and pass the resulting path in.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_for_test(db: crate::db::AppDatabase, data_dir: PathBuf) -> Self {
        use crate::acp::{EventBusMetrics, InternalEventBus};
        use crate::web::event_bridge::WebEventBroadcaster;

        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let metrics = Arc::new(EventBusMetrics::default());
        let acp_event_bus = Arc::new(InternalEventBus::new(metrics));
        let emitter = EventEmitter::web_only(broadcaster.clone(), acp_event_bus.clone());

        Self {
            db,
            connection_manager: default_connection_manager(),
            terminal_manager: default_terminal_manager(),
            event_broadcaster: broadcaster,
            acp_event_bus,
            emitter,
            data_dir,
            web_server_state: crate::web::WebServerState::new(),
            chat_channel_manager: default_chat_channel_manager(),
            workspace_transfer: Arc::new(
                crate::workspace_transfer::WorkspaceTransferManager::new_for_tests(
                    std::time::Duration::from_secs(60),
                ),
            ),
            pet_state: crate::pet_state_mapper::new_pet_state_handle(),
        }
    }
}
