use std::fs;

use insta::assert_snapshot;
use insta_cmd::assert_cmd_snapshot;

use crate::CliTest;

#[test]
fn update_and_use_configured_baseline() -> anyhow::Result<()> {
    let case = CliTest::with_files([
        ("pyproject.toml", "[tool.ty]\nbaseline = 'baseline.json'\n"),
        (
            "test.py",
            r#"
            from typing_extensions import reveal_type

            x: int = "wrong"
            reveal_type(x)
            "#,
        ),
    ])?;

    assert_cmd_snapshot!(case.command().arg("--update-baseline"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    info[revealed-type]: Revealed type
     --> test.py:5:13
      |
    5 | reveal_type(x)
      |             ^ `int`

    Found 1 diagnostic
    Updated baseline `<temp_dir>/baseline.json` with 1 diagnostic.

    ----- stderr -----
    ");

    let serialized = fs::read_to_string(case.root().join("baseline.json"))?;
    assert!(serialized.ends_with('\n'));
    assert_snapshot!(serialized, @r#"
    {
      "version": 0,
      "files": {
        "test.py": [
          {
            "rule": "invalid-assignment",
            "precedingHash": "33af5dcbb4264a903ea8237661d2e9d2",
            "followingHash": "9e9c1756bed4a360466ffdead18fa6a2"
          }
        ]
      }
    }
    "#);

    assert_cmd_snapshot!(case.command(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    info[revealed-type]: Revealed type
     --> test.py:5:13
      |
    5 | reveal_type(x)
      |             ^ `int`

    Found 1 diagnostic

    ----- stderr -----
    ");

    case.write_file(
        "test.py",
        r#"
        from typing_extensions import reveal_type

        x: int = "wrong"
        y: int = "new"
        reveal_type(x)
        "#,
    )?;
    assert_cmd_snapshot!(case.command().arg("--output-format=concise"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    test.py:5:10: error[invalid-assignment] Object of type `Literal["new"]` is not assignable to `int`
    test.py:6:13: info[revealed-type] Revealed type: `int`
    Found 2 diagnostics

    ----- stderr -----
    "#);

    assert_cmd_snapshot!(case.command().arg("--update-baseline"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    info[revealed-type]: Revealed type
     --> test.py:6:13
      |
    6 | reveal_type(x)
      |             ^ `int`

    Found 1 diagnostic
    Updated baseline `<temp_dir>/baseline.json` with 2 diagnostics.

    ----- stderr -----
    ");
    let updated = fs::read_to_string(case.root().join("baseline.json"))?;
    assert_cmd_snapshot!(case.command().arg("--update-baseline"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    info[revealed-type]: Revealed type
     --> test.py:6:13
      |
    6 | reveal_type(x)
      |             ^ `int`

    Found 1 diagnostic
    Updated baseline `<temp_dir>/baseline.json` with 2 diagnostics.

    ----- stderr -----
    ");
    assert_eq!(
        fs::read_to_string(case.root().join("baseline.json"))?,
        updated
    );
    Ok(())
}

#[test]
fn cli_baseline_path() -> anyhow::Result<()> {
    let case = CliTest::with_file("test.py", "x: int = 'wrong'\n")?;

    assert_cmd_snapshot!(case.command().args(["test.py", "--baseline", "state/baseline.json", "--update-baseline"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Updated baseline `<temp_dir>/state/baseline.json` with 1 diagnostic.

    ----- stderr -----
    ");
    assert!(case.root().join("state/baseline.json").is_file());

    assert_cmd_snapshot!(case.command().args(["--baseline", "state/baseline.json"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    All checks passed!

    ----- stderr -----
    ");
    Ok(())
}

#[test]
fn partial_updates_preserve_unchecked_files() -> anyhow::Result<()> {
    let case = CliTest::with_files([
        ("pyproject.toml", "[tool.ty]\nbaseline = 'baseline.json'\n"),
        ("nested/a.py", "a: int = 'wrong'\n"),
        ("nested/b.py", "b: int = 'wrong'\n"),
    ])?;
    assert_cmd_snapshot!(case.command().arg("--update-baseline"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Updated baseline `<temp_dir>/baseline.json` with 2 diagnostics.

    ----- stderr -----
    ");

    case.write_file("nested/a.py", "a: str = 1\n")?;
    assert_cmd_snapshot!(case.command().args(["nested/a.py", "--update-baseline"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Updated baseline `<temp_dir>/baseline.json` with 2 diagnostics.

    ----- stderr -----
    ");
    assert_cmd_snapshot!(case.command().arg("nested/a.py"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    All checks passed!

    ----- stderr -----
    ");

    // Even for a selected directory, excluded files must keep their baseline entries.
    case.write_file(
        "pyproject.toml",
        "[tool.ty]\nbaseline = 'baseline.json'\n[tool.ty.src]\nexclude = ['nested/b.py']\n",
    )?;
    case.write_file("nested/a.py", "a: str = 'fixed'\n")?;
    assert_cmd_snapshot!(case.command().args(["nested", "--update-baseline"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Updated baseline `<temp_dir>/baseline.json` with 1 diagnostic.

    ----- stderr -----
    ");
    let baseline = fs::read_to_string(case.root().join("baseline.json"))?;
    assert_snapshot!(baseline, @r#"
    {
      "version": 0,
      "files": {
        "nested/b.py": [
          {
            "rule": "invalid-assignment",
            "precedingHash": "6f98698e5bc8353e7a4c5d89deb7ff14",
            "followingHash": "12970f0291d38ddad0189c30e27ca443"
          }
        ]
      }
    }
    "#);

    case.write_file("pyproject.toml", "[tool.ty]\nbaseline = 'baseline.json'\n")?;
    assert_cmd_snapshot!(case.command(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    All checks passed!

    ----- stderr -----
    ");
    Ok(())
}

#[test]
fn partial_update_rejects_invalid_baseline() -> anyhow::Result<()> {
    let case = CliTest::with_files([
        ("pyproject.toml", "[tool.ty]\nbaseline = 'baseline.json'\n"),
        ("baseline.json", "{ not json"),
        ("test.py", "x: int = 'wrong'\n"),
    ])?;
    assert_cmd_snapshot!(case.command().args(["test.py", "--update-baseline"]), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    ty failed
      Cause: Failed to preserve entries for unchecked files; omit check paths to regenerate the entire baseline
      Cause: Failed to parse baseline `<temp_dir>/baseline.json`
      Cause: key must be a string at line 1 column 3
    ");
    assert_eq!(
        fs::read_to_string(case.root().join("baseline.json"))?,
        "{ not json"
    );

    case.write_file("baseline.json", r#"{"version":1,"files":{}}"#)?;
    assert_cmd_snapshot!(case.command().args(["test.py", "--update-baseline"]), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    ty failed
      Cause: Failed to preserve entries for unchecked files; omit check paths to regenerate the entire baseline
      Cause: Unsupported baseline version `1`
    ");
    assert_eq!(
        fs::read_to_string(case.root().join("baseline.json"))?,
        r#"{"version":1,"files":{}}"#
    );
    Ok(())
}

#[test]
fn update_requires_path_and_rejects_conflicts() -> anyhow::Result<()> {
    let case = CliTest::with_file("test.py", "x: int = 'wrong'\n")?;
    assert_cmd_snapshot!(case.command().arg("--update-baseline"), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    ty failed
      Cause: `--update-baseline` requires a baseline path from `--baseline` or configuration
    ");

    assert_cmd_snapshot!(case.command().args(["--baseline", "baseline.json", "--update-baseline", "--fix"]), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: the argument '--update-baseline' cannot be used with '--fix'

    Usage: ty check --baseline <PATH> --update-baseline [PATH]...

    For more information, try '--help'.
    ");
    Ok(())
}

#[test]
fn update_fails_when_non_baselineable_diagnostics_remain() -> anyhow::Result<()> {
    let case = CliTest::with_file("test.py", "if:\n")?;
    assert_cmd_snapshot!(case.command().args(["--baseline", "baseline.json", "--update-baseline"]), @"
    success: false
    exit_code: 1
    ----- stdout -----
    error[invalid-syntax]: Expected an expression
     --> test.py:1:3
      |
    1 | if:
      |   ^

    error[invalid-syntax]: Expected an indented block after `if` statement
     --> test.py:1:4
      |
    1 | if:
      |    ^

    Found 2 diagnostics
    Updated baseline `<temp_dir>/baseline.json` with 0 diagnostics.

    ----- stderr -----
    ");

    let baseline: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(case.root().join("baseline.json"))?)?;
    assert_eq!(baseline["files"], serde_json::json!({}));
    Ok(())
}

#[test]
fn invalid_baseline_is_reported_once() -> anyhow::Result<()> {
    let case = CliTest::with_files([
        ("pyproject.toml", "[tool.ty]\nbaseline = 'baseline.json'\n"),
        ("baseline.json", "{ not json"),
        ("test.py", "x: int = 'wrong'\n"),
    ])?;

    assert_cmd_snapshot!(case.command().arg("--output-format=concise"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    baseline.json:1:1: error[invalid-baseline] Failed to parse baseline `<temp_dir>/baseline.json`
    test.py:1:10: error[invalid-assignment] Object of type `Literal["wrong"]` is not assignable to `int`
    Found 2 diagnostics

    ----- stderr -----
    "#);

    // Update mode intentionally ignores the invalid old baseline and replaces it.
    assert_cmd_snapshot!(case.command().arg("--update-baseline"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Updated baseline `<temp_dir>/baseline.json` with 1 diagnostic.

    ----- stderr -----
    ");
    Ok(())
}

#[test]
fn baseline_json_is_path_keyed() -> anyhow::Result<()> {
    let case = CliTest::with_files([("b.py", "b: int = 'b'\n"), ("a.py", "a: int = 'a'\n")])?;
    assert_cmd_snapshot!(case.command().args(["--baseline", "baseline.json", "--update-baseline"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Updated baseline `<temp_dir>/baseline.json` with 2 diagnostics.

    ----- stderr -----
    ");

    let baseline = fs::read_to_string(case.root().join("baseline.json"))?;
    assert_snapshot!(baseline, @r#"
    {
      "version": 0,
      "files": {
        "a.py": [
          {
            "rule": "invalid-assignment",
            "precedingHash": "83c4a5b75d1fd7d8f418cc889df720b3",
            "followingHash": "cfd18b7b7e8b411ea044878c0e8b6771"
          }
        ],
        "b.py": [
          {
            "rule": "invalid-assignment",
            "precedingHash": "6f98698e5bc8353e7a4c5d89deb7ff14",
            "followingHash": "799da6a5d8e28b745c49494de187b60a"
          }
        ]
      }
    }
    "#);
    Ok(())
}
