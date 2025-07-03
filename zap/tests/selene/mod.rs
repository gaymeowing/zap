mod checker;

use std::{borrow::Cow, fs, sync::LazyLock};

use checker::{Selene, initialise_selene};
use insta::{Settings, assert_debug_snapshot, glob};
use selene_lib::{CheckerDiagnostic, lints::Severity};

static SELENE: LazyLock<Selene> = LazyLock::new(initialise_selene);

pub fn run_selene_test(input: &str, no_warnings: bool, insta_settings: &mut Settings, file_stem: Cow<'_, str>) {
	let code = zap::run(
		&format!("opt tooling = true\nopt types_output = \"network/types.luau\"\n{input}"),
		no_warnings,
	)
	.code
	.expect("Zap did not generate any code!");

	let client_ast = full_moon::parse_fallible(&code.client.code, SELENE.lua_version).into_ast();
	let client_diagnostics = SELENE
		.linter
		.test_on(&client_ast)
		.into_iter()
		.filter(|diagnostic| diagnostic.severity != Severity::Allow)
		.collect::<Vec<CheckerDiagnostic>>();

	insta_settings.set_snapshot_suffix(format!("{file_stem}@client"));
	insta_settings.bind(|| {
		assert_debug_snapshot!(client_diagnostics);
	});

	let server_ast = full_moon::parse_fallible(&code.server.code, SELENE.lua_version).into_ast();
	let server_diagnostics = SELENE
		.linter
		.test_on(&server_ast)
		.into_iter()
		.filter(|diagnostic| diagnostic.severity != Severity::Allow)
		.collect::<Vec<CheckerDiagnostic>>();

	insta_settings.set_snapshot_suffix(format!("{file_stem}@server"));
	insta_settings.bind(|| {
		assert_debug_snapshot!(server_diagnostics);
	});

	let types_ast = full_moon::parse_fallible(&code.types.as_ref().unwrap().code, SELENE.lua_version).into_ast();
	let types_diagnostics = SELENE
		.linter
		.test_on(&types_ast)
		.into_iter()
		.filter(|diagnostic| diagnostic.severity != Severity::Allow)
		.collect::<Vec<CheckerDiagnostic>>();
	insta_settings.set_snapshot_suffix(format!("{file_stem}@types"));
	insta_settings.bind(|| {
		assert_debug_snapshot!(types_diagnostics);
	});

	let tooling_ast = full_moon::parse_fallible(&code.tooling.as_ref().unwrap().code, SELENE.lua_version).into_ast();
	let tooling_diagnostics = SELENE
		.linter
		.test_on(&tooling_ast)
		.into_iter()
		.filter(|diagnostic| diagnostic.severity != Severity::Allow)
		.collect::<Vec<CheckerDiagnostic>>();

	insta_settings.set_snapshot_suffix(format!("{file_stem}@tooling"));
	insta_settings.bind(|| {
		assert_debug_snapshot!(tooling_diagnostics);
	});
}

#[test]
fn test_lints() {
	glob!(env!("CARGO_MANIFEST_DIR"), "tests/files/*.zap", |path| {
		let input = fs::read_to_string(path).unwrap();
		let file_stem = path.file_stem().unwrap().to_string_lossy();

		let mut insta_settings = Settings::new();
		insta_settings.set_prepend_module_to_snapshot(false);
		insta_settings.set_sort_maps(true);
		insta_settings.set_input_file(path);

		run_selene_test(&input, true, &mut insta_settings, file_stem)
	});
}
