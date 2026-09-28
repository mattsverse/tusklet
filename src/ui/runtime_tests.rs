use super::test_support::{click, workspace};
use super::*;
use gpui::TestAppContext;
use gpui_component::Root;

#[gpui::test]
fn empty_offline_workspace_can_select_podman(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    view.update(cx, |view, _| {
        view.snapshot.databases.clear();
        view.selected = None;
        view.snapshot.runtime_error = Some("Docker is not installed".into());
    });
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    click(cx, "runtime-podman");
    assert!(matches!(
        requests.try_recv().unwrap(),
        Request::Execute(Action::SelectRuntime(Engine::Podman))
    ));
    view.update(cx, |view, _| {
        assert!(view.busy.is_some());
        view.snapshot.runtime_engine = Engine::Podman;
        assert_eq!(view.runtime_status(), "● Podman unavailable");
        #[cfg(target_os = "macos")]
        assert!(format!("{:?}", view.tray_model()).contains("Podman unavailable"));
    });
}

#[gpui::test]
fn runtime_selection_protects_existing_databases_and_rechecks_stale_actions(
    cx: &mut TestAppContext,
) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    click(cx, "runtime-podman");
    assert!(requests.try_recv().is_err());
    view.update(cx, |view, cx| {
        view.select_runtime(Engine::Podman, cx);
        assert!(view.busy.is_none());
        view.snapshot.databases.clear();
        view.busy = Some("Creating database…".into());
        view.select_runtime(Engine::Podman, cx);
    });
    assert!(requests.try_recv().is_err());
}
