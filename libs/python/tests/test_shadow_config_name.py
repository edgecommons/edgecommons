"""The SHADOW config source's default shadow name (the component name) must be
sanitized to AWS IoT's allowed set ([A-Za-z0-9:_-]) — component names contain dots,
which AWS shadow names reject. Mirrors the Java/Rust/TS libraries."""
from edgecommons.config.manager.shadow_config_manager import _sanitize_shadow_name


def test_sanitizes_disallowed_characters():
    assert _sanitize_shadow_name("com.example.MyComponent") == "com_example_MyComponent"
    assert _sanitize_shadow_name("a.b/c+d#e f") == "a_b_c_d_e_f"


def test_leaves_valid_names_untouched():
    assert _sanitize_shadow_name("My-Shadow_1:v2") == "My-Shadow_1:v2"
