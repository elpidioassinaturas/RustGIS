//! Task re-entry stays bounded without changing finite scripts or outer transactions.
use super::*;

/// Create a locally trusted task through the public API, keeping the trust policy in the test.
fn task(s: &mut Session, name: &str, steps: Value) {
    s.run("task.new", &json!({"name": name, "steps": steps})).unwrap();
}

/// A script step that indirectly enters the named task, including a different task.
fn reenter(name: &str) -> Value {
    json!({"title":"again", "command":"script.run", "params":{"script":format!("task.run {}", json!({"task":name}))}})
}

/// A task cannot re-enter itself or another task through a script; cleanup permits a later run.
#[test]
fn task_reentry_through_script_is_refused_and_cleared() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    task(&mut s, "Loop", json!([reenter("Loop")]));
    task(&mut s, "Other", json!([]));
    task(&mut s, "Outer", json!([reenter("Other")]));
    for name in ["Loop", "Outer"] {
        let error = s.run("task.run", &json!({"task":name})).unwrap_err();
        assert!(error.contains("step 1") && error.contains("line 1") && error.contains("already running"), "{error}");
        assert!(!s.task_running);
        assert!(s.run("task.run", &json!({"task":"Other"})).is_ok());
    }
}

/// Successful script/task composition still changes the project in one undoable operation.
#[test]
fn task_reentry_guard_keeps_finite_scripts_and_single_undo() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    let before = s.map().name.clone();
    task(&mut s, "Finite", json!([{"title":"rename", "command":"script.run", "params":{"script":"map.rename {\"name\":\"Changed\"}"}}]));
    let undo = s.undo_depth();
    s.run("script.run", &json!({"script":"task.run {\"task\":\"Finite\"}"})).unwrap();
    assert_eq!(s.map().name, "Changed");
    assert_eq!(s.undo_depth(), undo + 1);
    assert!(!s.task_running);
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.map().name, before);
    s.run("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.map().name, "Changed");
    assert!(!s.task_running, "undo/redo do not restore execution state");
}

/// A refusal after an earlier project mutation restores the project without an extra undo.
#[test]
fn task_reentry_failure_rolls_back_outer_project_transaction() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    s.registry.commands.push(crate::CommandSpec::new("test.countEdit", "Count Edit", "Test", |s, _| {
        s.edit_counts.entry(7).or_default().modified += 1;
        Ok(Value::Null)
    }));
    task(
        &mut s,
        "Loop",
        json!([
            {"title":"rename", "command":"map.rename", "params":{"name":"Changed"}},
            {"title":"count", "command":"test.countEdit"},
            reenter("Loop")
        ]),
    );
    let undo = s.undo_depth();
    s.mark_saved();
    let before = serde_json::to_value(&s.project).unwrap();
    assert!(s.run("task.run", &json!({"task":"Loop"})).unwrap_err().contains("already running"));
    assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
    assert_eq!(s.undo_depth(), undo);
    assert!(s.edit_counts.is_empty());
    assert!(!s.dirty() && !s.task_running);
}

/// Invalid parameters and imported/direct-step refusals leave the guard usable and trust intact.
#[test]
fn task_reentry_guard_preserves_existing_refusals() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    task(&mut s, "Direct", json!([{"title":"again", "command":"task.run", "params":{"task":"Direct"}}]));
    s.run("task.new", &json!({"name":"Imported", "imported":true, "steps":[reenter("Direct")]})).unwrap();
    for (params, message) in [
        (json!({"task":"missing"}), "no task"),
        (json!({"task":"Direct", "step":999}), "steps"),
        (json!({"task":"Direct"}), "another task"),
        (json!({"task":"Imported"}), "from a file"),
    ] {
        let error = s.run("task.run", &params).unwrap_err();
        assert!(error.contains(message), "{error}");
        assert!(!s.task_running);
    }
}

/// Even a condition panic outside the step's own catch clears the outer task marker.
#[test]
fn task_reentry_guard_clears_after_condition_panic() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    s.registry.commands.push(
        crate::CommandSpec::new("test.conditionPanic", "Panic", "Test", |_, _| Ok(Value::Null)).when(|_| panic!("test condition panic")),
    );
    task(&mut s, "Panic", json!([{"title":"panic", "command":"test.conditionPanic"}]));
    let error = s.run("task.run", &json!({"task":"Panic"})).unwrap_err();
    assert!(error.contains("failed unexpectedly"), "{error}");
    assert!(!s.task_running);
    task(&mut s, "Fine", json!([]));
    s.run("task.run", &json!({"task":"Fine"})).unwrap();
}

/// Project replacement must not clear the active marker and allow a newly created task to enter.
#[test]
fn task_reentry_guard_survives_project_replacement() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    let script =
        format!("project.new {{\"name\":\"Replacement\"}}\ntask.new {}\ntask.run {{\"task\":\"New\"}}", json!({"name":"New", "steps":[]}));
    task(&mut s, "Original", json!([{"title":"replace", "command":"script.run", "params":{"script":script}}]));
    let before = serde_json::to_value(&s.project).unwrap();
    assert!(s.run("task.run", &json!({"task":"Original"})).unwrap_err().contains("already running"));
    assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
    assert!(!s.task_running);
}

/// A step-body panic also releases the marker, preserving the existing contextual task error.
#[test]
fn task_reentry_guard_clears_after_step_panic() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    s.registry.commands.push(crate::CommandSpec::new("test.stepPanic", "Panic", "Test", |_, _| panic!("test step panic")));
    task(&mut s, "Panic", json!([{"title":"panic", "command":"test.stepPanic"}]));
    let error = s.run("task.run", &json!({"task":"Panic"})).unwrap_err();
    assert!(error.contains("step 1") && error.contains("failed unexpectedly"), "{error}");
    assert!(!s.task_running);
    task(&mut s, "Fine", json!([]));
    s.run("task.run", &json!({"task":"Fine"})).unwrap();
}

/// Notebook cells keep their error-reporting contract when a task would indirectly enter tasks.
#[test]
fn task_reentry_guard_covers_notebook_paths() {
    isolate_plugins();
    let mut s = Session::new(Project::default());
    task(&mut s, "Loop", json!([reenter("Loop")]));
    s.run("notebook.new", &json!({"name":"Notebook"})).unwrap();
    s.run("notebook.addCell", &json!({"notebook":"Notebook", "source":"task.run {\"task\":\"Loop\"}"})).unwrap();
    s.run("notebook.run", &json!({"notebook":"Notebook"})).unwrap();
    assert!(s.project.notebooks[0].cells.iter().any(|c| c.error && c.output.as_deref().is_some_and(|o| o.contains("already running"))));
    task(&mut s, "ViaNotebook", json!([{"title":"notebook", "command":"notebook.run", "params":{"notebook":"Notebook"}}]));
    // A second run must produce its own cell error, rather than pass on the first run's output.
    for cell in &mut s.project.notebooks[0].cells {
        cell.output = None;
        cell.error = false;
    }
    let result = s.run("task.run", &json!({"task":"ViaNotebook"})).unwrap();
    assert!(result["ran"][0]["result"]["error"].is_u64(), "the task keeps the notebook's cell-error result: {result}");
    assert!(s.project.notebooks[0].cells.iter().any(|c| c.error && c.output.as_deref().is_some_and(|o| o.contains("already running"))));
    assert!(!s.task_running && !s.notebook_running);
}
