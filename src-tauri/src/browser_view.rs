//! Commands behind the browser panel: start and stop the live view of the
//! workspace's browser, forward the user's input, navigate, and pick an
//! element to send to the chat.

use crate::*;

use sinew_browser::view::{self, PickedElement, ViewInput, ViewTab};
use tokio::sync::watch;

pub(super) const BROWSER_FRAME_EVENT: &str = "browser-view-frame";

/// Running views, by workspace, with the id `browser_view_start` returned:
/// sending `true` stops the stream.
pub(super) type BrowserViews = Arc<Mutex<HashMap<String, (u64, watch::Sender<bool>)>>>;

static NEXT_VIEW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// One stream at a time per workspace: a tab has a single screencast, so a
/// new stream waits until the one it replaces has stopped it.
fn stream_lock(key: &str) -> Arc<Mutex<()>> {
    static LOCKS: std::sync::OnceLock<StdMutex<HashMap<String, Arc<Mutex<()>>>>> =
        std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap_or_else(|err| err.into_inner());
    locks.entry(key.to_string()).or_default().clone()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserViewInput {
    workspace_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserFramePayload {
    workspace_path: String,
    #[serde(flatten)]
    frame: view::ViewFrame,
}

fn session_key(workspace_path: &str) -> std::result::Result<String, String> {
    // The agent's tools key the browser by the normalized workspace root.
    normalize_workspace_root(workspace_path)
        .map(|root| root.display().to_string())
        .map_err(error_to_string)
}

/// Opens the browser if needed and streams its active tab to the panel.
/// Returns the view's id, for `browser_view_stop`.
#[tauri::command]
pub(super) async fn browser_view_start(
    app: AppHandle,
    state: State<'_, DesktopState>,
    input: BrowserViewInput,
) -> std::result::Result<u64, String> {
    let key = session_key(&input.workspace_path)?;
    view::ensure_session(&state.browser_sessions, &key)
        .await
        .map_err(error_to_string)?;
    let id = NEXT_VIEW.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (stop_tx, stop_rx) = watch::channel(false);
    if let Some((_, previous)) = state.browser_views.lock().await.insert(key.clone(), (id, stop_tx)) {
        let _ = previous.send(true);
    }
    let sessions = state.browser_sessions.clone();
    let views = state.browser_views.clone();
    let workspace_path = input.workspace_path.clone();
    let lock = stream_lock(&key);
    tauri::async_runtime::spawn(async move {
        let _turn = lock.lock().await;
        view::stream(sessions, key.clone(), stop_rx, move |frame| {
            let _ = app.emit(
                BROWSER_FRAME_EVENT,
                BrowserFramePayload {
                    workspace_path: workspace_path.clone(),
                    frame,
                },
            );
        })
        .await;
        // The browser closed: forget the view unless a newer one took over.
        let mut views = views.lock().await;
        if views.get(&key).is_some_and(|(current, _)| *current == id) {
            views.remove(&key);
        }
    });
    Ok(id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserViewStopInput {
    workspace_path: String,
    view_id: u64,
}

#[tauri::command]
pub(super) async fn browser_view_stop(
    state: State<'_, DesktopState>,
    input: BrowserViewStopInput,
) -> std::result::Result<(), String> {
    let key = session_key(&input.workspace_path)?;
    // Only the view this panel started: a stop that arrives after a newer
    // start (React re-running the effect) must not end the newer stream.
    let mut views = state.browser_views.lock().await;
    if views.get(&key).is_some_and(|(id, _)| *id == input.view_id) {
        if let Some((_, sender)) = views.remove(&key) {
            let _ = sender.send(true);
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserViewEventInput {
    workspace_path: String,
    event: ViewInput,
}

#[tauri::command]
pub(super) async fn browser_view_input(
    state: State<'_, DesktopState>,
    input: BrowserViewEventInput,
) -> std::result::Result<(), String> {
    let key = session_key(&input.workspace_path)?;
    view::send_input(&state.browser_sessions, &key, input.event)
        .await
        .map_err(error_to_string)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserViewNavigateInput {
    workspace_path: String,
    /// A URL, or `back`, `forward`, `reload`.
    target: String,
}

#[tauri::command]
pub(super) async fn browser_view_navigate(
    state: State<'_, DesktopState>,
    input: BrowserViewNavigateInput,
) -> std::result::Result<(), String> {
    let key = session_key(&input.workspace_path)?;
    view::navigate(&state.browser_sessions, &key, &input.target)
        .await
        .map_err(error_to_string)
}

#[tauri::command]
pub(super) async fn browser_view_tabs(
    state: State<'_, DesktopState>,
    input: BrowserViewInput,
) -> std::result::Result<Vec<ViewTab>, String> {
    let key = session_key(&input.workspace_path)?;
    view::tabs(&state.browser_sessions, &key)
        .await
        .map_err(error_to_string)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserViewTabInput {
    workspace_path: String,
    tab: String,
}

#[tauri::command]
pub(super) async fn browser_view_switch_tab(
    state: State<'_, DesktopState>,
    input: BrowserViewTabInput,
) -> std::result::Result<(), String> {
    let key = session_key(&input.workspace_path)?;
    view::switch_tab(&state.browser_sessions, &key, &input.tab)
        .await
        .map_err(error_to_string)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserViewPointInput {
    workspace_path: String,
    x: f64,
    y: f64,
    /// Also capture the element, to attach it to the chat.
    #[serde(default)]
    capture: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserPick {
    element: PickedElement,
    /// A PNG of the element, saved for the chat attachment.
    image_path: Option<String>,
}

/// The element under the pointer; with `capture`, also a picture of it.
#[tauri::command]
pub(super) async fn browser_view_pick(
    state: State<'_, DesktopState>,
    input: BrowserViewPointInput,
) -> std::result::Result<Option<BrowserPick>, String> {
    let key = session_key(&input.workspace_path)?;
    let Some(element) = view::element_at(&state.browser_sessions, &key, input.x, input.y)
        .await
        .map_err(error_to_string)?
    else {
        return Ok(None);
    };
    let image_path = if input.capture {
        let png = view::capture_region(&state.browser_sessions, &key, element.rect)
            .await
            .map_err(error_to_string)?;
        let dir = std::env::temp_dir().join("sinew-browser-picks");
        fs::create_dir_all(&dir).map_err(error_to_string)?;
        let path = dir.join(format!("element-{}.png", now_ms()));
        fs::write(&path, png).map_err(error_to_string)?;
        Some(path.display().to_string())
    } else {
        None
    };
    Ok(Some(BrowserPick {
        element,
        image_path,
    }))
}
