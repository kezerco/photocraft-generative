//! Hosted Generative Fill form. The Windows key draft is shared with the form's renderer,
//! never stored in its fields, undo history, preferences, or control protocol.

use craft::ui::contract::Bind;
use craft::ui::dialog::DialogId;
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, ParamDialog, ParamLayout, ParamResult};
use craft::ui::runtime::env::Label;
use photocraft_doc::DocId;
use serde_json::{Value, json};

use crate::{PhotocraftApp, dialogs::params, filter_dialog};

pub(crate) const COMMAND: &str = "edit.generativeFill";
pub(crate) const FAMILY: params::ParamFamily = params::ParamFamily {
    check: Some(check),
    cancel: Some(cancel),
    document: Some(document),
    ..params::ParamFamily::new(|key| key == COMMAND, filter_dialog::run)
};

fn document(value: &Value) -> Option<DocId> {
    value.as_u64().map(DocId)
}

pub(crate) fn open(app: &mut PhotocraftApp) -> Result<DialogId, String> {
    let (width, height, price) = photocraft_engine::generative_cmds::estimated_cost(&app.session).map_err(|e| e.to_string())?;
    let doc = app.session.active().ok_or("no active document")?.doc.id;
    let spec = photocraft_engine::commands::find(COMMAND).ok_or("Generative Fill is unavailable")?;
    let mut rows = filter_dialog::params(app, COMMAND, spec.params);
    for row in &mut rows {
        match row.key.as_str() {
            "confirmCharge" => row.label = Label::from("Agree to paid API request"),
            "acceptGuidedEdit" => row.label = Label::from("Accept a selection guide instead of an exact mask (FLUX.2 / Gemini)"),
            _ => {}
        }
    }
    let mut opening = filter_dialog::fields(app, COMMAND, &rows);
    opening.set("prompt", FieldValue::Text("Make it realistic".into()));
    #[cfg(windows)]
    let (draft, saved) = {
        clear_draft(app);
        (app.fal_key_draft.clone(), photocraft_engine::fal_key::has_saved_key())
    };
    let title = filter_dialog::title_of(spec.label);
    let reset = opening.clone();
    let id = params::open_param_unsaved(app, COMMAND, opening, json!(doc.0), move |fields| {
        #[cfg(windows)]
        let (draft, saved) = (draft.clone(), saved.clone());
        ParamDialog::command(COMMAND, title.clone(), rows.clone())
            .values(Bind::bound(fields))
            .reset_to(reset.clone())
            .layout(ParamLayout::Stacked)
            .previews(false)
            .below(move |ui, fields| {
                cost_note(ui, fields, width, height, price);
                #[cfg(windows)]
                key_entry(ui, &draft, &saved);
            })
    });
    Ok(id)
}

fn cost_note(ui: &mut egui::Ui, fields: &DialogFields, width: u32, height: u32, price: f64) {
    let price = match fields.get("model") {
        Some(FieldValue::Text(model)) if model == "flux2Pro" => {
            "FLUX.2 Pro Edit: from $0.03; input and output size may increase the price. Selection is a guide, not a mask.".into()
        }
        Some(FieldValue::Text(model)) if model == "geminiPro" => {
            "Gemini Pro Image Edit: approximately $0.15 per 1K result. Selection is a guide, not a mask.".into()
        }
        _ => format!("FLUX.1 Pro Fill: approximately ${price:.2}, based on square-padded input. Uses a selection mask."),
    };
    ui.label(format!("Crop {width} × {height} · {price}"));
    ui.label("Edge Pixels softly extends the edit past the selection. Set it to 0 for exact clipping.");
    ui.label("Only after you confirm will PhotoCraft send the crop, selection and prompt to fal.ai. Check current provider pricing.");
}

#[cfg(windows)]
fn key_entry(ui: &mut egui::Ui, draft: &std::sync::Mutex<String>, saved: &Result<bool, String>) {
    match saved {
        Ok(true) => ui.label("fal.ai API key saved for this Windows account. Leave blank to keep it."),
        Ok(false) => ui.label("Paste your fal.ai API key below. It will be saved for future launches."),
        Err(error) => ui.label(error),
    };
    ui.horizontal(|ui| {
        ui.label("fal.ai API key");
        let mut draft = draft.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ui.add(egui::TextEdit::singleline(&mut *draft).password(true).desired_width(220.0));
    });
    ui.small("Stored in Windows Credential Manager, not in PhotoCraft documents or preferences. FAL_KEY can also be set externally.");
}

fn check(app: &mut PhotocraftApp, result: &ParamResult) -> Result<(), Label> {
    let current = app.session.active().map(|state| state.doc.id);
    if current.is_none() || current != document(&params::captured(app, COMMAND)) {
        return Err(Label::from("The document changed. Reopen Generative Fill on the intended selection."));
    }
    let values = filter_dialog::command_params(COMMAND, &result.fields);
    if values.get("confirmCharge").and_then(Value::as_bool) != Some(true) {
        return Err(Label::from("Agree to the paid API request before continuing."));
    }
    if values.get("model").and_then(Value::as_str).is_some_and(|model| model != "flux1Fill")
        && values.get("acceptGuidedEdit").and_then(Value::as_bool) != Some(true)
    {
        return Err(Label::from("Confirm that this model uses the selection as a guide, not a mask."));
    }
    if values.get("prompt").and_then(Value::as_str).is_none_or(|p| p.trim().is_empty() || p.trim().len() > 2000) {
        return Err(Label::from("Enter a prompt of 1–2000 characters."));
    }
    photocraft_engine::generative_cmds::estimated_cost(&app.session).map_err(|e| Label::from(e.to_string()))?;
    #[cfg(windows)]
    {
        let mut draft = app.fal_key_draft.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !draft.is_empty() {
            photocraft_engine::fal_key::save_key(&draft).map_err(Label::from)?;
            draft.clear();
        }
    }
    photocraft_engine::fal_key::ensure_key().map_err(Label::from)
}

#[cfg(windows)]
fn clear_draft(app: &PhotocraftApp) {
    app.fal_key_draft.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

fn cancel(app: &mut PhotocraftApp, ctx: &egui::Context, id: DialogId) {
    #[cfg(windows)]
    clear_draft(app);
    filter_dialog::cancel(app, ctx, id);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn cancel_discards_uncommitted_key_without_serializing_it() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.run("file.new", json!({"width":32,"height":32})).unwrap();
        app.run("select.rect", json!({"x":3,"y":3,"width":8,"height":8})).unwrap();
        let id = open(&mut app).unwrap();
        *app.fal_key_draft.lock().unwrap() = "synthetic-key-never-save".into();
        let dialog = app.framework.dialogs.get_by_id::<craft::ui::patterns::form_dialogs::HostedParamDialog>(id).unwrap();
        assert!(!serde_json::to_string(dialog.fields()).unwrap().contains("synthetic-key-never-save"));
        let ctx = app.ctx().clone();
        crate::dialogs::cancel(&mut app, &ctx, id).unwrap();
        assert!(app.fal_key_draft.lock().unwrap().is_empty());
    }
}
