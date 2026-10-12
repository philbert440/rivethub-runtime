#[test]
fn counttype() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/counttype");
}

#[test]
fn customserde() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/customserde");
}

#[test]
fn dispatch() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/dispatch");
}

#[test]
fn dupnames() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/dupnames");
}

#[test]
fn duptype() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/duptype");
}

#[test]
fn dynvalue() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/dynvalue");
}

#[test]
fn jsmodel() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/jsmodel");
}

#[test]
fn manualderive() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/manualderive");
}

#[test]
fn naming() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/naming");
}

#[test]
fn nullable() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/nullable");
}

#[test]
fn opaque() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/opaque");
}

#[test]
fn redundantrename_fail() {
    dylint_testing::ui_test_example(env!("CARGO_PKG_NAME"), "redundantrenamefail");
}

#[test]
fn redundantrename_pass() {
    dylint_testing::ui_test_example(env!("CARGO_PKG_NAME"), "redundantrenamepass");
}

#[test]
fn strset() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/strset");
}

#[test]
fn tagfield() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/tagfield");
}

#[test]
fn typemacro() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/typemacro");
}

#[test]
fn untyped() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui/untyped");
}

#[test]
fn wirepub_fail() {
    dylint_testing::ui_test_example(env!("CARGO_PKG_NAME"), "wirefail");
}

#[test]
fn wirepub_pass() {
    dylint_testing::ui_test_example(env!("CARGO_PKG_NAME"), "wirepass");
}
