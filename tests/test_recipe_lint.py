from ggcommons_cli.recipe_lint import lint_recipe_text


def test_clean_recipe_has_no_problems():
    assert lint_recipe_text('ComponentName: "com.example.X"\nComponentVersion: "1.0.0"\n') == []


def test_flags_gdk_component_name_placeholder():
    problems = lint_recipe_text('ComponentName: "{COMPONENT_NAME}"\n')
    assert any("COMPONENT_NAME" in p for p in problems)


def test_flags_artifact_permissions_block():
    recipe = "Artifacts:\n  - URI: s3://b/x\n    Permissions:\n      Read: ALL\n"
    assert any("Permissions" in p for p in lint_recipe_text(recipe))


def test_flags_leftover_placeholder():
    assert any("placeholder" in p for p in lint_recipe_text("ComponentPublisher: <<AUTHOR>>\n"))


def test_runtime_component_name_var_is_not_flagged():
    # The ggcommons runtime var {ComponentName} (camelCase) is fine; only GDK's
    # all-caps {COMPONENT_NAME} is a problem.
    assert lint_recipe_text('topic: "heartbeat/{ThingName}/{ComponentName}"\n') == []
