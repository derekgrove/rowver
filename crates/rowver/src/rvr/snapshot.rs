use super::{checksum, digest, object, required, string};
pub(super) use crate::json::canonical;
use crate::{invalid, Result};
use serde_json::Value;
use std::collections::HashSet;

/// Validate recipe text and its independently recorded snapshot checksums.
pub(super) fn validate(made: &Value) -> Result<()> {
    for field in ["study", "built", "by"] {
        string(made, field)?;
    }
    object(made, "files")?;
    let cart = object(made, "cart")?;
    if digest(string(cart, "source")?.as_bytes()) != checksum(cart)? {
        return Err(invalid(
            "made.cart.sha256 does not match the saved recipe source",
        ));
    }
    if let Some(modules) = cart.get("modules") {
        let modules = modules
            .as_object()
            .ok_or_else(|| invalid("cart.modules must be an object"))?;
        for (path, module) in modules {
            if digest(string(module, "source")?.as_bytes()) != checksum(module)? {
                return Err(invalid(format!(
                    "Saved module {path} has an invalid source checksum"
                )));
            }
        }
    }
    let mut body = made.clone();
    body.as_object_mut().unwrap().remove("sha256");
    if digest(canonical(&body)?.as_bytes()) != checksum(made)? {
        return Err(invalid(
            "made.sha256 does not match the complete recipe snapshot",
        ));
    }
    if let Some(hooks) = cart.get("hooks") {
        checksum(&serde_json::json!({"sha256":string(hooks, "keep")?}))?;
        required(hooks, "columns")?
            .as_object()
            .ok_or_else(|| invalid("cart.hooks.columns must be an object"))?;
    }
    Ok(())
}

/// Ensure saved hook marks cover precisely the checkpoint's named columns.
pub(super) fn columns(made: &Value, columns: &[Value]) -> Result<()> {
    let cart = object(made, "cart")?;
    let Some(hooks) = cart.get("hooks") else {
        return Ok(());
    };
    let marks = object(hooks, "columns")?.as_object().unwrap();
    let names: HashSet<&str> = columns
        .iter()
        .map(|column| string(column, "name"))
        .collect::<Result<_>>()?;
    if marks.len() != names.len() || marks.keys().any(|name| !names.contains(name.as_str())) {
        return Err(invalid("Saved column hook marks do not match data.cols"));
    }
    for mark in marks.values() {
        checksum(&serde_json::json!({"sha256":mark}))?;
    }
    for field in ["prepare", "metadata"] {
        if let Some(mark) = hooks.get(field) {
            checksum(&serde_json::json!({"sha256":mark}))?;
        }
    }
    Ok(())
}
