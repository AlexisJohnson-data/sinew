//! Raw Chrome DevTools Protocol calls.
//!
//! chromiumoxide 0.7 ships typed commands generated from an older protocol;
//! a response carrying an enum value it does not know (a new AX property, a
//! new resource type…) fails to decode and the whole call errors. The
//! commands this crate depends on go through `call` instead, which sends
//! the method by name and reads the answer as plain JSON.

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use chromiumoxide::{Command, Method, Page};
use serde::{Serialize, Serializer};
use serde_json::Value;

#[derive(Debug)]
struct Raw {
    method: &'static str,
    params: Value,
}

impl Serialize for Raw {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.params.serialize(serializer)
    }
}

impl Method for Raw {
    fn identifier(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.method)
    }
}

impl Command for Raw {
    type Response = Value;
}

/// Sends `method` with `params` to the page's target and returns the result.
pub(crate) async fn call(page: &Page, method: &'static str, params: Value) -> Result<Value> {
    page.execute(Raw { method, params })
        .await
        .map(|response| response.result)
        .map_err(|err| anyhow!("{method}: {err}"))
}

/// `Runtime.evaluate` returning the value by value (`undefined` → null).
pub(crate) async fn eval(page: &Page, expression: &str) -> Result<Value> {
    let result = call(
        page,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
    .await?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(anyhow!("{}", exception_text(details)));
    }
    Ok(result["result"]["value"].clone())
}

/// Runs `function` with `this` bound to the DOM node `backend_node_id`;
/// `args` are passed by value. Returns the function's value.
pub(crate) async fn call_on_node(
    page: &Page,
    backend_node_id: i64,
    function: &str,
    args: &[Value],
) -> Result<Value> {
    let object_id = resolve_node(page, backend_node_id).await?;
    let result = call(
        page,
        "Runtime.callFunctionOn",
        serde_json::json!({
            "objectId": object_id,
            "functionDeclaration": function,
            "arguments": args.iter().map(|value| serde_json::json!({ "value": value })).collect::<Vec<_>>(),
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
    .await;
    let _ = call(
        page,
        "Runtime.releaseObject",
        serde_json::json!({ "objectId": object_id }),
    )
    .await;
    let result = result?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(anyhow!("{}", exception_text(details)));
    }
    Ok(result["result"]["value"].clone())
}

/// A remote object id for a DOM node, or an error when the node is gone.
pub(crate) async fn resolve_node(page: &Page, backend_node_id: i64) -> Result<String> {
    let resolved = call(
        page,
        "DOM.resolveNode",
        serde_json::json!({ "backendNodeId": backend_node_id }),
    )
    .await?;
    resolved["object"]["objectId"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("node {backend_node_id} has no remote object"))
}

pub(crate) fn exception_text(details: &Value) -> String {
    let description = details["exception"]["description"]
        .as_str()
        .or_else(|| details["text"].as_str())
        .unwrap_or("JavaScript exception");
    description.lines().next().unwrap_or(description).to_string()
}
