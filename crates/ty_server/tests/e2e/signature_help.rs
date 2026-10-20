use anyhow::Result;
use lsp_types::{Documentation, MarkupKind, Position};
use ruff_db::system::SystemPath;
use ty_server::ClientOptions;

use crate::TestServerBuilder;

/// Tests that we get signature help even when the cursor
/// is on the function name.
///
/// This is a regression test to ensure we don't accidentally
/// cause this case to stop working.
#[test]
fn works_in_function_name() -> Result<()> {
    let workspace_root = SystemPath::new("src");
    let foo = SystemPath::new("src/foo.py");
    let foo_content = "\
import re
re.match('', '')
";

    let mut server = TestServerBuilder::new()?
        .with_initialization_options(&ClientOptions::default())
        .with_workspace(workspace_root, None)?
        .with_file(foo, foo_content)?
        .build()
        .wait_until_workspaces_are_initialized();

    server.open_text_document(foo, foo_content, 1);

    let signature_help = server.signature_help_request(&server.file_uri(foo), Position::new(1, 6));

    insta::assert_json_snapshot!(signature_help);

    Ok(())
}

#[test]
fn documentation_uses_the_clients_preferred_format() -> Result<()> {
    let workspace_root = SystemPath::new("src");
    let path = SystemPath::new("src/foo.py");
    let source = "\
def describe(name: str):
    \"\"\"<p>Describe a name.</p>

    Args:
        name: A <code>file_name</code>.
    \"\"\"
    pass

describe(
";

    for (formats, markdown, tags) in [
        (None, false, &[][..]),
        (Some(vec![]), false, &[][..]),
        (
            Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
            false,
            &["p", "code"][..],
        ),
        (
            Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
            true,
            &[][..],
        ),
        (Some(vec![MarkupKind::Markdown]), true, &["p", "code"][..]),
    ] {
        let mut server = TestServerBuilder::new()?
            .with_workspace(workspace_root, None)?
            .with_file(path, source)?
            .with_signature_documentation_format(formats.clone())
            .with_allowed_html_tags(tags)
            .build()
            .wait_until_workspaces_are_initialized();
        server.open_text_document(path, source, 1);

        let help = server
            .signature_help_request(&server.file_uri(path), Position::new(8, 9))
            .expect("signature help");
        let signature = &help.signatures[0];
        let parameter = &signature.parameters.as_ref().expect("parameters")[0];
        if markdown {
            let Some(Documentation::MarkupContent(documentation)) = &signature.documentation else {
                panic!("Expected Markdown signature documentation for {formats:?}");
            };
            assert_eq!(documentation.kind, MarkupKind::Markdown);
            let (summary, expected_parameter) = if tags.is_empty() {
                (
                    "&lt;p&gt;Describe a name.&lt;/p&gt;",
                    "A &lt;code&gt;file\\_name&lt;/code&gt;.",
                )
            } else {
                ("<p>Describe a name.</p>", "A <code>file&#95;name</code>.")
            };
            assert!(documentation.value.contains(summary));
            assert!(documentation.value.contains(expected_parameter));

            let Some(Documentation::MarkupContent(documentation)) = &parameter.documentation else {
                panic!("Expected Markdown parameter documentation for {formats:?}");
            };
            assert_eq!(documentation.kind, MarkupKind::Markdown);
            assert_eq!(documentation.value, expected_parameter);
        } else {
            assert!(matches!(
                signature.documentation,
                Some(Documentation::String(_))
            ));
            assert_eq!(
                parameter.documentation,
                Some(Documentation::String(
                    "A <code>file_name</code>.".to_string()
                ))
            );
        }
    }
    Ok(())
}
