use super::{AppWindow, Theme};
use i_slint_backend_testing::{
    mock_elapsed_time, AccessibleRole, ElementHandle, ElementQuery, TestingBackend,
    TestingBackendOptions,
};
use slint::{ComponentHandle, PhysicalSize};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const PAGES: [&str; 5] = [
    "\u{670d}\u{52a1}\u{8fde}\u{63a5}",
    "\u{6a21}\u{578b}\u{4e0e}\u{5ba2}\u{6237}\u{7aef}",
    "\u{4ee3}\u{7406}\u{4e0e}\u{8fd0}\u{884c}",
    "\u{73af}\u{5883}\u{4e0e} Hooks",
    "\u{8fd0}\u{884c}\u{65e5}\u{5fd7}",
];

const ENGLISH_PAGES: [&str; 5] = [
    "Connection",
    "Models & Clients",
    "Proxy & Runtime",
    "Environment & Hooks",
    "Logs",
];

fn button(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .unwrap_or_else(|| panic!("Missing button: {label}"))
}

fn click(ui: &AppWindow, label: &str) {
    button(ui, label).mock_single_click(slint::platform::PointerEventButton::Left);
    mock_elapsed_time(Duration::from_millis(300));
}

#[test]
fn language_switch_updates_labels_without_resetting_settings() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("bilingual window");
    ui.on_translate_message(|message| super::localization::translate_message(&message).into());
    ui.set_server_port("5050".into());
    ui.set_proxy_url("127.0.0.1:2080".into());
    ui.set_github_token("fixture-token".into());
    ui.set_status_text("Log cleared".into());
    ui.set_log_text("Original upstream diagnostic".into());
    ui.invoke_select_page(2);
    ui.set_is_chinese(false);
    ui.show().expect("headless language selector");
    mock_elapsed_time(Duration::from_millis(300));
    assert_eq!(
        button(&ui, "Start Server").accessible_label().as_deref(),
        Some("Start Server")
    );
    assert_eq!(ui.get_localized_status().as_str(), "Log cleared");
    let selector = ElementHandle::find_by_element_id(&ui, "AppWindow::language-selector")
        .next()
        .expect("language selector");
    assert_eq!(selector.accessible_value().as_deref(), Some("English"));
    let selected = Rc::new(Cell::new(None));
    let selected_language = selected.clone();
    ui.on_language_changed(move |chinese| selected_language.set(Some(chinese)));
    selector.mock_single_click(slint::platform::PointerEventButton::Left);
    for key in [slint::platform::Key::UpArrow, slint::platform::Key::Return] {
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: key.into() });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: key.into() });
    }
    mock_elapsed_time(Duration::from_millis(300));
    assert_eq!(
        selected.get(),
        Some(true),
        "selection must fire the persistence callback"
    );
    assert!(ui.get_is_chinese());
    assert_eq!(
        selector.accessible_value().as_deref(),
        Some("\u{4e2d}\u{6587}")
    );
    assert_eq!(
        button(&ui, "\u{542f}\u{52a8}\u{670d}\u{52a1}")
            .accessible_label()
            .as_deref(),
        Some("\u{542f}\u{52a8}\u{670d}\u{52a1}")
    );
    assert_eq!(
        ui.get_localized_status().as_str(),
        "\u{65e5}\u{5fd7}\u{5df2}\u{6e05}\u{7a7a}"
    );
    assert_eq!(ui.get_active_page(), 2);
    assert_eq!(ui.get_server_port().as_str(), "5050");
    assert_eq!(ui.get_proxy_url().as_str(), "127.0.0.1:2080");
    assert_eq!(ui.get_github_token().as_str(), "fixture-token");
    assert_eq!(ui.get_log_text().as_str(), "Original upstream diagnostic");
    ui.hide().expect("close language fixture");
}

#[test]
fn branded_gui_renders_and_controls_work_at_supported_sizes() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .expect("isolated software renderer");
    let ui = AppWindow::new().expect("branded window");
    ui.on_translate_message(|message| super::localization::translate_message(&message).into());
    ui.set_server_port("5050".into());
    ui.set_available_models(
        Rc::new(slint::VecModel::from(vec![
            "claude-sonnet-4-6".into(),
            "claude-haiku-4-5".into(),
        ]))
        .into(),
    );
    ui.set_deps_summary("[OK] Ready to use".into());
    ui.set_deps_line1("Copilot API Server: [OK] Embedded".into());
    ui.set_deps_line2("VS Code: [OK] 1.104.3".into());
    ui.set_deps_line3("Extensions: [OK]".into());
    ui.set_log_text("[INFO] Local rendering fixture\n[INFO] No network requests".into());
    ui.show().expect("headless window");

    let screenshots =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/branding-screenshots");
    std::fs::create_dir_all(&screenshots).expect("screenshot directory");
    for (width, height) in [(1120, 800), (800, 620)] {
        ui.window().set_size(PhysicalSize::new(width, height));
        for chinese in [true, false] {
            ui.set_is_chinese(chinese);
            let pages = if chinese { PAGES } else { ENGLISH_PAGES };
            for dark in [false, true] {
                ui.global::<Theme>().set_dark(dark);
                mock_elapsed_time(Duration::from_millis(300));
                for (page, label) in pages.into_iter().enumerate() {
                    click(&ui, label);
                    assert_eq!(ui.get_active_page(), page as i32);
                    assert_eq!(ui.get_server_port().as_str(), "5050");
                    let snapshot = ui.window().take_snapshot().expect("software screenshot");
                    assert_eq!((snapshot.width(), snapshot.height()), (width, height));
                    let pixels = snapshot.as_slice();
                    let gold_pixels = (12..72)
                        .flat_map(|vertical| {
                            (12..112)
                                .map(move |horizontal| (vertical * width + horizontal) as usize)
                        })
                        .filter(|&offset| {
                            let pixel = pixels[offset];
                            pixel.r > 220 && pixel.g > 120 && pixel.g < 220 && pixel.b < 80
                        })
                        .count();
                    assert!(gold_pixels > 500, "original logo must render on every page");
                    let accent = pixels[(83 * width + width / 2) as usize];
                    assert_eq!((accent.r, accent.g, accent.b), (255, 176, 0));
                    for control in ElementQuery::from_root(&ui)
                        .match_accessible_role(AccessibleRole::Button)
                        .find_all()
                    {
                        let position = control.absolute_position();
                        let size = control.size();
                        assert!(
                            position.x >= -1.0
                                && position.y >= -1.0
                                && position.x + size.width <= width as f32 + 1.0
                                && position.y + size.height <= height as f32 + 1.0
                                && size.width >= 16.0
                                && size.height >= 16.0,
                            "button outside window: {:?}, {position:?}, {size:?}",
                            control.accessible_label()
                        );
                    }
                    let theme = if dark { "dark" } else { "light" };
                    let language = if chinese { "zh" } else { "en" };
                    for toggle in ElementQuery::from_root(&ui)
                        .match_accessible_role(AccessibleRole::Switch)
                        .find_all()
                    {
                        let rail = toggle
                            .query_descendants()
                            .match_id("Switch::rail")
                            .find_first()
                            .expect("switch rail");
                        let label = toggle
                            .query_descendants()
                            .match_id("Switch::label")
                            .find_first()
                            .expect("switch label");
                        assert!(
                            rail.absolute_position().x + rail.size().width + 8.0
                                <= label.absolute_position().x,
                            "switch rail must not overlap its label"
                        );
                    }
                    image::save_buffer(
                        screenshots.join(format!(
                            "{width}x{height}-{language}-{theme}-page-{page}.png"
                        )),
                        snapshot.as_bytes(),
                        width,
                        height,
                        image::ColorType::Rgba8,
                    )
                    .expect("PNG screenshot");
                }
            }
        }
    }

    ui.set_is_chinese(true);
    mock_elapsed_time(Duration::from_millis(300));
    let copied = Rc::new(Cell::new(0));
    let copied_count = copied.clone();
    ui.on_copy_log(move || copied_count.set(copied_count.get() + 1));
    click(&ui, "\u{590d}\u{5236}\u{65e5}\u{5fd7}");
    assert_eq!(copied.get(), 1, "tooltip must not swallow icon clicks");

    let cleared = Rc::new(Cell::new(0));
    let cleared_count = cleared.clone();
    ui.on_clear_log(move || cleared_count.set(cleared_count.get() + 1));
    click(&ui, "\u{6e05}\u{7a7a}\u{65e5}\u{5fd7}");
    assert_eq!(cleared.get(), 1);

    ui.set_log_text("".into());
    mock_elapsed_time(Duration::from_millis(300));
    button(&ui, "\u{590d}\u{5236}\u{65e5}\u{5fd7}").invoke_accessible_default_action();
    assert_eq!(copied.get(), 1, "disabled actions must not fire");

    let theme_switch = ElementHandle::find_by_accessible_label(&ui, "\u{6df1}\u{8272}")
        .find(|element| element.accessible_role() == Some(AccessibleRole::Switch))
        .expect("theme switch");
    theme_switch.mock_single_click(slint::platform::PointerEventButton::Left);
    mock_elapsed_time(Duration::from_millis(300));
    assert!(!ui.global::<Theme>().get_dark());
    click(&ui, PAGES[2]);
    let proxy_switch =
        ElementHandle::find_by_accessible_label(&ui, "\u{542f}\u{7528}\u{4ee3}\u{7406}")
            .find(|element| element.accessible_role() == Some(AccessibleRole::Switch))
            .expect("proxy switch");
    proxy_switch.mock_single_click(slint::platform::PointerEventButton::Left);
    mock_elapsed_time(Duration::from_millis(300));
    assert!(ui.get_use_proxy());

    ui.set_provider("azure".into());
    ui.set_azure_enabled(true);
    click(&ui, PAGES[0]);
    for chinese in [true, false] {
        ui.set_is_chinese(chinese);
        mock_elapsed_time(Duration::from_millis(300));
        let azure = ui.window().take_snapshot().expect("small Azure layout");
        let language = if chinese { "zh" } else { "en" };
        image::save_buffer(
            screenshots.join(format!("800x620-{language}-light-azure.png")),
            azure.as_bytes(),
            azure.width(),
            azure.height(),
            image::ColorType::Rgba8,
        )
        .expect("Azure screenshot");
    }
    ui.hide().expect("close headless window");
}

#[cfg(windows)]
#[test]
fn windows_icon_has_all_sizes_and_decodes() {
    let icon = std::fs::read(std::path::Path::new(env!("OUT_DIR")).join("app-icon.ico"))
        .expect("generated Windows icon");
    assert_eq!(&icon[..6], &[0, 0, 1, 0, 8, 0]);
    let decoded = image::load_from_memory_with_format(&icon, image::ImageFormat::Ico)
        .expect("valid ICO pixels");
    assert_eq!((decoded.width(), decoded.height()), (256, 256));
}
