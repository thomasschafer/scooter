use frep_core::validation::SearchConfiguration;
use scooter::{
    headless::run_headless, test_with_both_regex_modes,
    test_with_both_regex_modes_and_fixed_strings,
};

mod utils;

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_basic_replacement,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file.",
                "It contains TEST_PATTERN that should be replaced.",
                "Multiple lines with TEST_PATTERN here.",
            ),
            "file2.txt" => text!(
                "Another file with TEST_PATTERN.",
                "Second line.",
            ),
            "subdir/file3.txt" => text!(
                "Nested file with TEST_PATTERN.",
                "Only one occurrence here.",
            ),
            "binary.bin" => &[10, 19, 3, 92],
        );

        let search_config = SearchConfiguration {
            search_text: "TEST_PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 3 files updated".to_owned(),);

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a test file.",
                "It contains REPLACEMENT that should be replaced.",
                "Multiple lines with REPLACEMENT here.",
            ),
            "file2.txt" => text!(
                "Another file with REPLACEMENT.",
                "Second line.",
            ),
            "subdir/file3.txt" => text!(
                "Nested file with REPLACEMENT.",
                "Only one occurrence here.",
            ),
            "binary.bin" => &[10, 19, 3, 92],
        );

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_headless_regex_replacement,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "Numbers: 123, 456, and 789.",
                "Phone: (555) 123-4567",
                "IP: 192.168.1.1",
            ),
            "file2.txt" => text!(
                "User IDs: user_123, admin_456, guest_789",
                "Codes: ABC-123, DEF-456",
            ),
            "subdir/file3.txt" => text!(
                "Parameters: param1=value1, param2=value2",
                "Configuration: config_123=setting",
            ),
        );

        let search_config = SearchConfiguration {
            search_text: r"\d{3}",
            replacement_text: "XXX",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings: false,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 3 files updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "Numbers: XXX, XXX, and XXX.",
                "Phone: (XXX) XXX-XXX7",
                "IP: XXX.XXX.1.1",
            ),
            "file2.txt" => text!(
                "User IDs: user_XXX, admin_XXX, guest_XXX",
                "Codes: ABC-XXX, DEF-XXX",
            ),
            "subdir/file3.txt" => text!(
                "Parameters: param1=value1, param2=value2",
                "Configuration: config_XXX=setting",
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_headless_regex_with_capture_groups,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "users.txt" => text!(
                "username: john_doe, email: john@example.com",
                "username: jane_smith, email: jane@example.com",
            ),
            "logs.txt" => text!(
                "[2023-01-15] INFO: System started",
                "[2023-02-20] ERROR: Connection failed",
            ),
        );

        let search_config = SearchConfiguration {
            search_text: r"username: (\w+), email: ([^@]+)@",
            replacement_text: "user: $1 (contact: $2 at",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings: false,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated".to_string(),);

        let search_config = SearchConfiguration {
            search_text: r"\[(\d{4})-(\d{2})-(\d{2})\]",
            replacement_text: "[$3/$2/$1]",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("logs.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings: false,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "users.txt" => text!(
                "user: john_doe (contact: john atexample.com",
                "user: jane_smith (contact: jane atexample.com",
            ),
            "logs.txt" => text!(
                "[15/01/2023] INFO: System started",
                "[20/02/2023] ERROR: Connection failed",
            ),
        );

        Ok(())
    }
);

#[tokio::test]
async fn test_headless_advanced_regex_features() -> anyhow::Result<()> {
    let temp_dir = create_test_files!(
        "code.rs" => text!(
            "let x = 10;",
            "const y: i32 = 20;",
            "let mut z = 30;",
            "const MAX_SIZE: usize = 100;",
        ),
        "text.md" => text!(
            "# Heading 1",
            "## Subheading",
            "This is **bold** and *italic* text.",
        ),
        "data.csv" => text!(
            "id,name,value",
            "1,item1,100",
            "2,item2,200",
            "3,item3,300",
        ),
    );

    // Negative lookahead - match 'let' but not 'let mut'
    let search_config = SearchConfiguration {
        search_text: r"let(?!\s+mut)",
        replacement_text: "const",
        directory: temp_dir.path().to_path_buf(),
        include_globs: Some("code.rs"),
        exclude_globs: Some(""),
        include_hidden: false,
        fixed_strings: false,
        match_case: true,
        match_whole_word: false,
        advanced_regex: true,
    };

    let result = run_headless(search_config);
    assert_eq!(result.unwrap(), "Success: 1 file updated".to_string(),);

    // Positive lookbehind - match numbers after headings
    let search_config = SearchConfiguration {
        search_text: r"(?<=# )[A-Za-z]+\s+(\d+)",
        replacement_text: "Section $1",
        directory: temp_dir.path().to_path_buf(),
        include_globs: Some("*.md"),
        exclude_globs: Some(""),
        include_hidden: false,
        fixed_strings: false,
        match_case: true,
        match_whole_word: false,
        advanced_regex: true,
    };

    let result = run_headless(search_config);
    assert_eq!(result.unwrap(), "Success: 1 file updated".to_string(),);

    // Add spaces after commas in CSV file
    let search_config = SearchConfiguration {
        search_text: ",",
        replacement_text: ", ",
        directory: temp_dir.path().to_path_buf(),
        include_globs: Some("*.csv"),
        exclude_globs: Some(""),
        include_hidden: false,
        fixed_strings: true,
        match_case: true,
        match_whole_word: false,
        advanced_regex: true,
    };

    let result = run_headless(search_config);
    assert_eq!(result.unwrap(), "Success: 1 file updated".to_string(),);

    assert_test_files!(
        &temp_dir,
        "code.rs" => text!(
            "const x = 10;",
            "const y: i32 = 20;",
            "let mut z = 30;",
            "const MAX_SIZE: usize = 100;",
        ),
        "text.md" => text!(
            "# Section 1",
            "## Subheading",
            "This is **bold** and *italic* text.",
        ),
        "data.csv" => text!(
            "id, name, value",
            "1, item1, 100",
            "2, item2, 200",
            "3, item3, 300",
        ),
    );

    Ok(())
}

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_with_globs,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "src/main.rs" => text!(
                "fn main() { println!(\"REPLACE_ME\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"REPLACE_ME\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACE_ME\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACE_ME\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `REPLACE_ME`"
            ),
            "build/output.txt" => text!(
                "Build output: REPLACE_ME"
            ),
        );

        // Include glob - only match Rust files
        let search_config = SearchConfiguration {
            search_text: "REPLACE_ME",
            replacement_text: "REPLACED_CODE",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.rs"),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 4 files updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "fn main() { println!(\"REPLACED_CODE\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"REPLACED_CODE\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `REPLACE_ME`"
            ),
            "build/output.txt" => text!(
                "Build output: REPLACE_ME"
            ),
        );

        // Exclude glob - exclude test files
        let search_config = SearchConfiguration {
            search_text: "REPLACED_CODE",
            replacement_text: "FINAL_VERSION",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.rs"),
            exclude_globs: Some("tests/**"),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "fn main() { println!(\"FINAL_VERSION\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"FINAL_VERSION\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `REPLACE_ME`"
            ),
            "build/output.txt" => text!(
                "Build output: REPLACE_ME"
            ),
        );

        // Multiple include globs
        let search_config = SearchConfiguration {
            search_text: "REPLACE_ME",
            replacement_text: "DOCS_REPLACED",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.md,**/*.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "fn main() { println!(\"FINAL_VERSION\"); }"
            ),
            "src/lib.rs" => text!(
                "pub fn lib_fn() { println!(\"FINAL_VERSION\"); }"
            ),
            "tests/test1.rs" => text!(
                "#[test] fn test1() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "tests/test2.rs" => text!(
                "#[test] fn test2() { assert_eq!(\"REPLACED_CODE\", \"expected\"); }"
            ),
            "docs/readme.md" => text!(
                "# Documentation",
                "",
                "Example: `DOCS_REPLACED`"
            ),
            "build/output.txt" => text!(
                "Build output: DOCS_REPLACED"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_match_whole_word,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "words.txt" => text!(
                "This has whole_word and whole_word_suffix and prefix_whole_word.",
                "Also xwhole_wordx and sub_whole_word_part."
            ),
            "code.rs" => text!(
                "let whole_word = 10;",
                "let my_whole_word_var = 20;",
                "let whole_word_end = 30;"
            ),
        );

        let search_config = SearchConfiguration {
            search_text: "whole_word",
            replacement_text: "REPLACED",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: true,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "words.txt" => text!(
                "This has REPLACED and whole_word_suffix and prefix_whole_word.",
                "Also xwhole_wordx and sub_whole_word_part."
            ),
            "code.rs" => text!(
                "let REPLACED = 10;",
                "let my_whole_word_var = 20;",
                "let whole_word_end = 30;"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_case_sensitivity,
    |advanced_regex, fixed_strings| async move {
        // Case sensitive test
        let temp_dir1 = create_test_files!(
            "case.txt" => text!(
                "This has pattern, PATTERN, and PaTtErN variations.",
                "Also pAtTeRn and Pattern."
            ),
            "example.rs" => text!(
                "let pattern = 10;",
                "let PATTERN = 20;",
                "let Pattern = 30;"
            ),
        );

        let search_config = SearchConfiguration {
            search_text: "pattern",
            replacement_text: "REPLACED",
            directory: temp_dir1.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        assert_test_files!(
            &temp_dir1,
            "case.txt" => text!(
                "This has REPLACED, PATTERN, and PaTtErN variations.",
                "Also pAtTeRn and Pattern."
            ),
            "example.rs" => text!(
                "let REPLACED = 10;",
                "let PATTERN = 20;",
                "let Pattern = 30;"
            ),
        );

        // Case insensitive test
        let temp_dir2 = create_test_files!(
            "case.txt" => text!(
                "This has pattern, PATTERN, and PaTtErN variations.",
                "Also pAtTeRn and Pattern."
            ),
            "example.rs" => text!(
                "let pattern = 10;",
                "let PATTERN = 20;",
                "let Pattern = 30;"
            ),
        );

        let search_config = SearchConfiguration {
            search_text: "pattern",
            replacement_text: "variable",
            directory: temp_dir2.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: false,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        assert_test_files!(
            &temp_dir2,
            "case.txt" => text!(
                "This has variable, variable, and variable variations.",
                "Also variable and variable."
            ),
            "example.rs" => text!(
                "let variable = 10;",
                "let variable = 20;",
                "let variable = 30;"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_binary_files_skipped,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "text.txt" => text!(
                "This is a text file with PATTERN."
            ),
            "binary.bin" => b"This is a binary file with PATTERN".as_slice(),
            "image.png" => &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );

        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "text.txt" => text!(
                "This is a text file with REPLACEMENT."
            ),
            "binary.bin" => b"This is a binary file with PATTERN".as_slice(),
            "image.png" => &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_output_formatting,
    |advanced_regex, fixed_strings| async move {
        // Verify headless code formats output correctly
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "One PATTERN here",
                "Another PATTERN here",
                "And a third PATTERN here",
            ),
            "file2.txt" => text!(
                "Just one PATTERN in this file",
            ),
            "file3.txt" => text!(
                "No patterns in this file",
            ),
        );

        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "One REPLACEMENT here",
                "Another REPLACEMENT here",
                "And a third REPLACEMENT here",
            ),
            "file2.txt" => text!(
                "Just one REPLACEMENT in this file",
            ),
            "file3.txt" => text!(
                "No patterns in this file",
            ),
        );

        // Test with whole word matching
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "PATTERN and PATTERN_1"
            ),
            "file2.txt" => text!(
                "PATTERN and PATTERN_2"
            ),
        );

        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: true,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "REPLACEMENT and PATTERN_1"
            ),
            "file2.txt" => text!(
                "REPLACEMENT and PATTERN_2"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_hidden_files,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "visible.txt" => text!(
                "This is a visible file with PATTERN"
            ),
            ".hidden.txt" => text!(
                "This is a hidden file with PATTERN"
            ),
            ".config/settings.txt" => text!(
                "Settings file with PATTERN"
            ),
        );

        // Default behavior - hidden files excluded
        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false, // Default behavior
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 1 file updated".to_string(),);

        // Only visible file should be modified, hidden files untouched
        assert_test_files!(
            &temp_dir,
            "visible.txt" => text!(
                "This is a visible file with REPLACEMENT"
            ),
            ".hidden.txt" => text!(
                "This is a hidden file with PATTERN"
            ),
            ".config/settings.txt" => text!(
                "Settings file with PATTERN"
            ),
        );

        // Explicit include hidden files
        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: true, // Include hidden files
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_string(),);

        // Now all files should be modified
        assert_test_files!(
            &temp_dir,
            "visible.txt" => text!(
                "This is a visible file with REPLACEMENT"
            ),
            ".hidden.txt" => text!(
                "This is a hidden file with REPLACEMENT"
            ),
            ".config/settings.txt" => text!(
                "Settings file with REPLACEMENT"
            ),
        );

        Ok(())
    }
);

test_with_both_regex_modes!(
    test_headless_validation_errors_regex,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        let search_config = SearchConfiguration {
            search_text: "(", // Unclosed parenthesis = invalid regex
            replacement_text: "replacement",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings: false,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("Failed to parse search text"));

        assert_test_files!(
            &temp_dir,
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_validation_errors_glob,
    |advanced_regex, fixed_strings: bool| async move {
        let temp_dir = create_test_files!(
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        let search_config = SearchConfiguration {
            search_text: "valid",
            replacement_text: "replacement",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("{{"), // Invalid glob pattern
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("glob"));

        assert_test_files!(
            &temp_dir,
            "test.txt" => text!(
                "This file won't be modified as the configuration will be invalid"
            )
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_file_globs_include,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with PATTERN in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with PATTERN something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *PATTERN* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with PATTERN",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        // Test include glob - only include .txt files
        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("*.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_owned(),);

        // Verify only .txt files were modified
        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with REPLACEMENT in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with REPLACEMENT something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *PATTERN* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with PATTERN",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        let result = run_headless(SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("*.txt"),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        });
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 0 files updated".to_owned(),);

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_file_globs_exclude,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with PATTERN in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with PATTERN something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *PATTERN* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with PATTERN",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        // Test exclude glob - exclude .txt files
        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some("*.txt"),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_owned(),);

        // Verify non-.txt files were modified
        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "# Sample Text File 1",
                "",
                "This is text file 1 with PATTERN in the middle of the content.",
                "Some additional text for context.",
                "The end of the file."
            ),
            "file2.txt" => text!(
                "Another Text File",
                "This is text file 2 with PATTERN something.",
                "Some more text on a separate line.",
                "And a little more text for additional context.",
                "Final line of the file."
            ),
            "other.md" => text!(
                "# Markdown Document",
                "",
                "## Introduction",
                "",
                "This is markdown with *REPLACEMENT* in it.",
                "",
                "* Item 1",
                "* Item 2",
                "* Item 3"
            ),
            "code.rs" => text!(
                "// Sample Rust code file",
                "",
                "fn main() {",
                "    // Comment with REPLACEMENT",
                "    let x = 42;",
                "    println!(\"The answer is {}\", x);",
                "}"
            )
        );

        Ok(())
    }
);

// Test combined include and exclude globs
test_with_both_regex_modes_and_fixed_strings!(
    test_headless_file_globs_combined,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "src/main.rs" => text!(
                "//! Main entry point for the application",
                "",
                "use std::io;",
                "",
                "fn main() {",
                "    println!(\"Hello, this contains PATTERN to replace\");",
                "    ",
                "    let result = process_data();",
                "    println!(\"Result: {}\", result);",
                "}",
                "",
                "fn process_data() -> i32 {",
                "    // Processing with PATTERN",
                "    42",
                "}"
            ),
            "src/lib.rs" => text!(
                "//! Library functionality",
                "",
                "/// Public function that contains PATTERN in documentation",
                "pub fn lib_fn() {",
                "    // Implementation details",
                "    let value = internal_helper();",
                "    println!(\"Value: {}\", value);",
                "}",
                "",
                "fn internal_helper() -> &'static str {",
                "    \"PATTERN in a string literal\"",
                "}"
            ),
            "src/utils.rs" => text!(
                "//! Utility functions",
                "",
                "pub fn util() {",
                "    // Using PATTERN in a comment",
                "    println!(\"Utility function\");",
                "}",
                "",
                "pub fn format_data(input: &str) -> String {",
                "    // Format containing PATTERN",
                "    format!(\"[formatted]: {}\", input)",
                "}"
            ),
            "tests/test.rs" => text!(
                "//! Test module",
                "",
                "#[cfg(test)]",
                "mod tests {",
                "    use super::*;",
                "",
                "    #[test]",
                "    fn test_basic_functionality() {",
                "        // Test with PATTERN that shouldn't be changed",
                "        assert_eq!(\"PATTERN\", \"expected\".replace(\"expected\", \"PATTERN\"));",
                "    }",
                "}"
            ),
            "docs/readme.md" => text!(
                "# Project Documentation",
                "",
                "## Overview",
                "",
                "This project does things with PATTERN processing.",
                "",
                "## Examples",
                "",
                "```rust",
                "// Example code with PATTERN",
                "let x = process_pattern();",
                "```",
                "",
                "## API Reference",
                "",
                "See the inline documentation for details."
            )
        );

        // Include all .rs files but exclude those in tests directory
        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACEMENT",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some("**/*.rs"),
            exclude_globs: Some("tests/**"),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 3 files updated".to_owned(),);

        // Verify only source .rs files were modified, not test files or docs
        assert_test_files!(
            &temp_dir,
            "src/main.rs" => text!(
                "//! Main entry point for the application",
                "",
                "use std::io;",
                "",
                "fn main() {",
                "    println!(\"Hello, this contains REPLACEMENT to replace\");",
                "    ",
                "    let result = process_data();",
                "    println!(\"Result: {}\", result);",
                "}",
                "",
                "fn process_data() -> i32 {",
                "    // Processing with REPLACEMENT",
                "    42",
                "}"
            ),
            "src/lib.rs" => text!(
                "//! Library functionality",
                "",
                "/// Public function that contains REPLACEMENT in documentation",
                "pub fn lib_fn() {",
                "    // Implementation details",
                "    let value = internal_helper();",
                "    println!(\"Value: {}\", value);",
                "}",
                "",
                "fn internal_helper() -> &'static str {",
                "    \"REPLACEMENT in a string literal\"",
                "}"
            ),
            "src/utils.rs" => text!(
                "//! Utility functions",
                "",
                "pub fn util() {",
                "    // Using REPLACEMENT in a comment",
                "    println!(\"Utility function\");",
                "}",
                "",
                "pub fn format_data(input: &str) -> String {",
                "    // Format containing REPLACEMENT",
                "    format!(\"[formatted]: {}\", input)",
                "}"
            ),
            "tests/test.rs" => text!(
                "//! Test module",
                "",
                "#[cfg(test)]",
                "mod tests {",
                "    use super::*;",
                "",
                "    #[test]",
                "    fn test_basic_functionality() {",
                "        // Test with PATTERN that shouldn't be changed",
                "        assert_eq!(\"PATTERN\", \"expected\".replace(\"expected\", \"PATTERN\"));",
                "    }",
                "}"
            ),
            "docs/readme.md" => text!(
                "# Project Documentation",
                "",
                "## Overview",
                "",
                "This project does things with PATTERN processing.",
                "",
                "## Examples",
                "",
                "```rust",
                "// Example code with PATTERN",
                "let x = process_pattern();",
                "```",
                "",
                "## API Reference",
                "",
                "See the inline documentation for details."
            )
        );

        Ok(())
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_headless_binary_detection,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = create_test_files!(
            "contains_binary.txt" => binary!(
                b"Some content PATTERN in a file",
                b"with \xFF invalid PATTERN UTF-8",
                b"and some PATTERN valid UTF-8 too.",
            ),
            "text.txt" => text!(
                "Regular text file with PATTERN to replace.",
                "Some more text with various words",
                "and another usage of PATTERN here"
            ),
            "completely_binary.txt" => binary!(
                b"\x89PNG\r\n\x1a\n",
                b"\x00\x00\x00\rIHDR",
                b"\x00\x00PATTERN \x00\x01\x00\x00\x00\x01",
                b"\x08\x02\x00\x00\x00",
                b"\x90wS\xde",
                b"\x00\x00\x00\x0cIDAT",
                b"x\x9cc```\x00\x00\x00\x04\x00\x01",
                b"\r\n\x03PATTERN \xb8",
                b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"
            ),
            "binary_extension.pdf" => text!(
                "This content PATTERN shouldn't matter",
                "as PDFs should be skipped PATTERN by extension, without",
                "even reading the file.",
            ),
        );

        let search_config = SearchConfiguration {
            search_text: "PATTERN",
            replacement_text: "REPLACED",
            directory: temp_dir.path().to_path_buf(),
            include_globs: Some(""),
            exclude_globs: Some(""),
            include_hidden: false,
            fixed_strings,
            match_case: true,
            match_whole_word: false,
            advanced_regex,
        };

        let result = run_headless(search_config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success: 2 files updated".to_owned(),);

        assert_test_files!(
            &temp_dir,
            "contains_binary.txt" => binary!(
                b"Some content REPLACED in a file",
                b"with \xFF invalid PATTERN UTF-8",
                b"and some REPLACED valid UTF-8 too.",
            ),
            "text.txt" => text!(
                "Regular text file with REPLACED to replace.",
                "Some more text with various words",
                "and another usage of REPLACED here"
            ),
            "completely_binary.txt" => binary!(
                b"\x89PNG\r\n\x1a\n",
                b"\x00\x00\x00\rIHDR",
                b"\x00\x00PATTERN \x00\x01\x00\x00\x00\x01",
                b"\x08\x02\x00\x00\x00",
                b"\x90wS\xde",
                b"\x00\x00\x00\x0cIDAT",
                b"x\x9cc```\x00\x00\x00\x04\x00\x01",
                b"\r\n\x03PATTERN \xb8",
                b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"
            ),
            "binary_extension.pdf" => text!(
                "This content PATTERN shouldn't matter",
                "as PDFs should be skipped PATTERN by extension, without",
                "even reading the file.",
            ),
        );

        Ok(())
    }
);
