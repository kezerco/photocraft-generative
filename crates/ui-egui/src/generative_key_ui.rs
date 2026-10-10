//! Windows fal.ai key entry. Drafts stay in memory outside the serializable dialog fields;
//! saved credentials belong to this Windows user's Credential Manager, not to PhotoCraftData.

use serde_json::{Map, Value};

use crate::PhotocraftApp;

pub(crate) fn owns(fields: &Map<String, Value>) -> bool {
    fields.get("__command").and_then(Value::as_str) == Some("edit.generativeFill")
}

pub(crate) fn opened(app: &mut PhotocraftApp) {
    app.fal_key_draft.clear();
    app.fal_key_saved = Some(photocraft_engine::fal_key::has_saved_key());
}

pub(crate) fn body(app: &mut PhotocraftApp, ui: &mut egui::Ui, fields: &mut Map<String, Value>) {
    crate::filter_dialog::body(ui, fields);
    ui.add_space(8.0);
    match app.fal_key_saved.as_ref() {
        Some(Ok(true)) => {
            ui.label("fal.ai API key: saved securely for this Windows account. Leave blank to keep it.");
        }
        Some(Ok(false)) => {
            ui.label("fal.ai API key: paste your key below; it will be saved for future launches.");
        }
        Some(Err(error)) => {
            ui.label(error);
            ui.label("You may still use FAL_KEY if Windows Credential Manager is unavailable.");
        }
        None => {}
    }
    ui.horizontal(|ui| {
        ui.label("fal.ai API key");
        ui.add(egui::TextEdit::singleline(&mut app.fal_key_draft).password(true).desired_width(220.0).hint_text("Paste new key (optional if saved)"));
    });
    ui.small("Stored by Windows Credential Manager, not in PhotoCraft documents or preferences. Only sent to fal.ai after a paid request is confirmed.");
}

pub(crate) fn confirm(app: &mut PhotocraftApp, fields: &Map<String, Value>) -> Result<Value, String> {
    if !app.fal_key_draft.is_empty() {
        let key = std::mem::take(&mut app.fal_key_draft);
        photocraft_engine::fal_key::save_key(&key)?;
        app.fal_key_saved = Some(Ok(true));
    }
    let params = crate::filter_dialog::params_of(fields);
    app.run("edit.generativeFill", params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_discards_uncommitted_key_without_saving() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        let fields = serde_json::from_value::<Map<String, Value>>(serde_json::json!({"__command":"edit.generativeFill"})).unwrap();
        let id = app.ui.open_dialog(crate::state::DialogKind::Command, fields);
        app.fal_key_draft = "secret-never-save".into();
        crate::dialogs::cancel(&mut app, id).unwrap();
        assert!(app.fal_key_draft.is_empty());
    }

    #[test]
    fn key_is_not_a_command_parameter_or_dialog_field() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.fal_key_draft = "secret-never-serialize".into();
        let fields =
            serde_json::from_value::<Map<String, Value>>(serde_json::json!({"__command":"edit.generativeFill", "prompt":"Make it realistic"})).unwrap();
        let params = crate::filter_dialog::params_of(&fields);
        assert!(!serde_json::to_string(&params).unwrap().contains("secret-never-serialize"));
        assert!(!serde_json::to_string(&app.ui).unwrap().contains("secret-never-serialize"));
    }
}
