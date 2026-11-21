/// Source capabilities for one file path suffix, matching `libraries/sourceanalysis/sourcekind`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceFormat {
    pub kind: &'static str,
    pub language: Option<&'static str>,
    pub jsx: bool,
    pub outline: bool,
    pub semantic_resolver: Option<&'static str>,
}

const fn code(language: &'static str, jsx: bool, resolver: &'static str) -> SourceFormat {
    SourceFormat {
        kind: "code",
        language: Some(language),
        jsx,
        outline: true,
        semantic_resolver: Some(resolver),
    }
}

const SUFFIX_FORMATS: &[(&str, SourceFormat)] = &[
    (".tsx", code("typescript", true, "typescript")),
    (".mts", code("typescript", false, "typescript")),
    (".cts", code("typescript", false, "typescript")),
    (".ts", code("typescript", false, "typescript")),
    (".jsx", code("javascript", true, "typescript")),
    (".mjs", code("javascript", false, "typescript")),
    (".cjs", code("javascript", false, "typescript")),
    (".js", code("javascript", false, "typescript")),
    (".pyi", code("python", false, "python")),
    (".py", code("python", false, "python")),
    (".go", code("go", false, "gopls")),
    (".rs", code("rust", false, "rust")),
    (
        ".md",
        SourceFormat {
            kind: "markdown",
            language: None,
            jsx: false,
            outline: true,
            semantic_resolver: None,
        },
    ),
    (
        ".json",
        SourceFormat {
            kind: "json",
            language: None,
            jsx: false,
            outline: true,
            semantic_resolver: Some("typescript"),
        },
    ),
];

/// Classify a file path by exact case-sensitive suffix.
///
/// Note: `.d.ts`, `.d.mts`, `.d.cts` end in `.ts`, `.mts`, `.cts` respectively,
/// so they are automatically classified as TypeScript just like Go's `sourcekind.Classify`.
pub fn classify(path: &str) -> Option<SourceFormat> {
    for &(suffix, format) in SUFFIX_FORMATS {
        if path.ends_with(suffix) {
            return Some(format);
        }
    }
    None
}
