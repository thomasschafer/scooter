#!/usr/bin/env nu

use std repeat

def create_test_files [dir: string] {
    let files = [
        {path: "file1.txt", content: "This is before text in file1\nAnother line with before\n"},
        {path: "file2.txt", content: "No match here\nJust some text\n"},
        {path: "file3.rs", content: "fn before() {\n    println!(\"before\");\n}\n\n\n"},
        {path: "subdir/file4.txt", content: "before at start\nMiddle before text\nbefore at end"},
        {path: "subdir/file5.txt", content: "Nothing to replace here"},
        {path: "subdir/file6.txt", content: ""},
        {path: "subdir/subsubdir/file7.txt", content: "\n\r\n"},
        {path: "file8.txt", content: "Some before text\r\nMore text   \r\n   foo bar baz   \r\n  \rbeforeafter\n "},
        {path: "file9.txt", content: "foo\r\nbefore\r\nbaz\r\n"},
    ]

    for file in $files {
        let filepath = ($dir | path join $file.path)
        let parent_dir = ($filepath | path dirname)

        if not ($parent_dir | path exists) {
            mkdir $parent_dir
        }

        $file.content | save -f $filepath
    }
}

def cleanup_directories [dirs: list<string>] {
    for dir in $dirs {
        try { rm -rf $dir }
    }
}

def tool_to_dirname [name: string] {
    $"output-($name | str replace --all ' + ' '-' | str replace --all ' ' '-')"
}

def run_tool [base_dir: string, name: string, command: string] {
    let tool_dir = tool_to_dirname $name

    print $"Running ($name)"

    cp -r $base_dir $tool_dir

    let previous_dir = $env.PWD
    cd $tool_dir

    ^expect -c $"spawn bash -c \"($command)\"; expect eof" | complete

    cd $previous_dir

    $tool_dir
}

def compare_directories [dir1: string, dir2: string, name1: string, name2: string] {
    print $"Comparing ($name1) vs ($name2)"

    let diff_result = (^diff -r $dir1 $dir2 | complete)
    let directories_match = ($diff_result.exit_code == 0)

    if $directories_match {
        print $"✅ PASSED: ($name1) and ($name2) produced identical results"
    } else {
        print $"❌ FAILED: ($name1) and ($name2) produced different results"
        print "Differences found:"
        print $diff_result.stdout
    }

    $directories_match
}

def get_tools [scooter_binary: string, search_term: string, replace_term: string] {
    [
        {
            name: "scooter",
            command: $"($scooter_binary) -X -s '($search_term)' -r '($replace_term)'",
        },
        {
            name: "scooter (--no-tui)",
            command: $"($scooter_binary) -N -s '($search_term)' -r '($replace_term)'",
        },
        {
            name: "ripgrep + sd",
            command: $"rg -l ($search_term) | xargs sd '($search_term)' '($replace_term)'",
        },
        {
            name: "fastmod",
            command: $"fastmod --accept-all '($search_term)' '($replace_term)'",
        },
        {
            name: "fd + sd",
            command: $"fd --type file --exec sd '($search_term)' '($replace_term)'",
        },
    ]
}

def compare_results [tool_results: list] {
    let scooter_directory = ($tool_results | where name == "scooter" | get dir.0)
    mut all_tests_passed = true

    for result in ($tool_results | where name != "scooter") {
        let comparison_passed = (compare_directories $scooter_directory $result.dir "scooter" $result.name)
        if not $comparison_passed {
            $all_tests_passed = false
        }
    }

    $all_tests_passed
}

def get_benchmark_repo_path [repo_url: string] {
    let cache_dir = ($nu.home-path | path join ".cache" "scooter" "benchmark")
    let repo_name = $repo_url | path basename | str replace ".git" ""

    let repo_path = ($cache_dir | path join $repo_name)

    if not ($cache_dir | path exists) {
        mkdir $cache_dir
    }

    if not ($repo_path | path exists) {
        print $"Downloading ($repo_name) to cache..."
        ^git clone --depth 1 $repo_url $repo_path
    } else {
        print $"Using cached ($repo_name)"
    }

    $repo_path
}

def setup_test_data [test_dir: string, repo_url: string = ""] {
    if ($repo_url | is-not-empty) {
        get_benchmark_repo_path $repo_url
    } else {
        mkdir $test_dir
        create_test_files $test_dir
        $test_dir
    }
}

def update_readme_benchmark [project_dir: string, benchmark_file: string] {
    let benchmark_table = (open $benchmark_file)
    let readme_path = ($project_dir | path join "README.md")
    let readme_content = (open $readme_path)

    let start_marker = "<!-- BENCHMARK START -->"
    let end_marker = "<!-- BENCHMARK END -->"

    let lines_content = ($readme_content | lines)
    mut start_idx = -1
    mut end_idx = -1

    for i in 0..<($lines_content | length) {
        if ($lines_content | get $i | str contains $start_marker) {
            $start_idx = $i
        } else if ($lines_content | get $i | str contains $end_marker) {
            $end_idx = $i
            break
        }
    }

    if $start_idx >= 0 and $end_idx >= 0 {
        let before_lines = ($lines_content | take ($start_idx + 1))
        let after_lines = ($lines_content | skip $end_idx)
        let new_content = ($before_lines | append $benchmark_table | append $after_lines | append "" | str join "\n")

        $new_content | save -f $readme_path

        print "Results embedded in README.md"
        true
    } else {
        print "❌ Could not find benchmark markers in README.md"
        false
    }
}

def run_benchmark [project_dir: string, search: string, replace: string, scooter_binary: string, update_readme: bool, repo_url: string = ""] {
    print "Running benchmark..."

    let actual_repo_url = if ($repo_url | is-not-empty) { $repo_url } else { "https://github.com/torvalds/linux.git" }
    let benchmark_source = get_benchmark_repo_path $actual_repo_url
    let benchmark_dir = ($project_dir | path join "benchmark-temp")

    let benchmark_tools = get_tools $scooter_binary $search $replace

    const benchmark_file = "benchmark-results.md"
    mut hyperfine_args = [
        "--prepare" $"rm -rf ($benchmark_dir); cp -r ($benchmark_source) ($benchmark_dir)"
        "--cleanup" $"rm -rf ($benchmark_dir)"
        "--export-markdown" $benchmark_file
        "--warmup" "1"
        "--min-runs" "5"
    ]

    for tool in $benchmark_tools {
        $hyperfine_args = ($hyperfine_args | append [
            "--command-name" $tool.name
            $"expect -c 'spawn bash -c \"cd ($benchmark_dir) && ($tool.command)\"; expect eof'"
        ])
    }

    print "Running hyperfine benchmark..."
    ^hyperfine ...$hyperfine_args
    let benchmark_exit_code = $env.LAST_EXIT_CODE

    if $benchmark_exit_code == 0 and ($benchmark_file | path exists) {
        print "✅ Benchmark completed successfully"
        if $update_readme {
            update_readme_benchmark $project_dir $benchmark_file
        }
        rm $benchmark_file
    } else {
        print "❌ Benchmark failed"
    }

    $benchmark_exit_code
}

def run_e2e_tests [replacement_dir: string, all_tools: list, repo_url: string = ""] {
    print "Running end-to-end tests..."

    let test_source_dir = setup_test_data $replacement_dir $repo_url

    let tool_results = $all_tools | each {|tool|
        {
            name: $tool.name,
            dir: (run_tool $test_source_dir $tool.name $tool.command),
        }
    }
    let all_tests_passed = compare_results $tool_results

    if $all_tests_passed {
        print "✅ ALL TESTS PASSED"
        0
    } else {
        print "❌ SOME TESTS FAILED"
        1
    }
}

def test_stdin_processing [scooter_binary: string] {
    print "Testing stdin processing..."

    # Test basic stdin replacement
    let test_input = "hello world foo bar"
    let result1 = (do { echo $test_input | ^$scooter_binary --no-tui -s "foo" -r "baz" } | complete)

    if $result1.exit_code != 0 or ($result1.stdout != "hello world baz bar") {
        print "❌ FAILED: scooter basic stdin processing failed"
        print $"Exit code: ($result1.exit_code)"
        print $"Stderr: ($result1.stderr)"
        print $"Stdout: ($result1.stdout)"
        return 1
    }

    # Test regex with stdin
    let result2 = (do { echo "123 456 789" | ^$scooter_binary --no-tui -s '\d{3}' -r 'XXX' } | complete)
    if $result2.exit_code != 0 or ($result2.stdout != "XXX XXX XXX") {
        print "❌ FAILED: scooter regex stdin processing failed"
        print $"Exit code: ($result2.exit_code)"
        print $"Stderr: ($result2.stderr)"
        print $"Stdout: ($result2.stdout)"
        return 1
    }

    # Test case insensitive with stdin
    let result3 = (do { echo "Hello WORLD" | ^$scooter_binary --no-tui -s 'hello' -r 'hi' --case-insensitive } | complete)
    if $result3.exit_code != 0 or ($result3.stdout != "hi WORLD") {
        print "❌ FAILED: scooter case insensitive stdin processing failed"
        print $"Exit code: ($result3.exit_code)"
        print $"Stderr: ($result3.stderr)"
        print $"Stdout: ($result3.stdout)"
        return 1
    }

    # Test whole word matching with stdin
    let result4 = (do { echo "test_word and test" | ^$scooter_binary --no-tui -s 'test' -r 'exam' --match-whole-word } | complete)
    if $result4.exit_code != 0 or ($result4.stdout != "test_word and exam") {
        print "❌ FAILED: scooter whole word stdin processing failed"
        print $"Exit code: ($result4.exit_code)"
        print $"Stderr: ($result4.stderr)"
        print $"Stdout: ($result4.stdout)"
        return 1
    }

    # Test multi-line stdin processing
    let multiline_input = "line one with foo\nline two with bar\nline three with foo again"
    let result5 = (do { echo $multiline_input | ^$scooter_binary --no-tui -s "foo" -r "baz" } | complete)
    let expected_output = "line one with baz\nline two with bar\nline three with baz again"
    if $result5.exit_code != 0 or ($result5.stdout != $expected_output) {
        print "❌ FAILED: scooter multi-line stdin processing failed"
        print $"Exit code: ($result5.exit_code)"
        print $"Expected: ($expected_output)"
        print $"Actual: ($result5.stdout)"
        return 1
    }

    # Test empty stdin
    let result6 = (do { echo "" | ^$scooter_binary --no-tui -s "foo" -r "bar" } | complete)
    if $result6.exit_code != 0 or ($result6.stdout != "") {
        print "❌ FAILED: scooter empty stdin processing failed"
        print $"Exit code: ($result6.exit_code)"
        print $"Stderr: ($result6.stderr)"
        print $"Stdout: ($result6.stdout)"
        return 1
    }

    # Test stdin with no matches
    let result7 = (do { echo "no matches here" | ^$scooter_binary --no-tui -s "foo" -r "bar" } | complete)
    if $result7.exit_code != 0 or ($result7.stdout != "no matches here") {
        print "❌ FAILED: scooter no-match stdin processing failed"
        print $"Exit code: ($result7.exit_code)"
        print $"Stderr: ($result7.stderr)"
        print $"Stdout: ($result7.stdout)"
        return 1
    }

    # Test stdin with long lines (potential memory issues)
    let long_line = ('x' | repeat 1000 | str join) + "foo" + ('y' | repeat 1000 | str join)
    let expected_long = ('x' | repeat 1000 | str join) + "bar" + ('y' | repeat 1000 | str join)
    let result8 = (do { echo $long_line | ^$scooter_binary --no-tui -s "foo" -r "bar" } | complete)
    if $result8.exit_code != 0 or ($result8.stdout != $expected_long) {
        print "❌ FAILED: scooter long line stdin processing failed"
        print $"Exit code: ($result8.exit_code)"
        print $"Stderr: ($result8.stderr)"
        return 1
    }

    print "✅ PASSED: correctly processes stdin input"
    0
}

def test_stdin_tui_mode [scooter_binary: string] {
    print "Testing stdin processing in TUI mode..."

    # Test TUI mode with immediate search and replace (-X flag)
    let result1 = (^expect -c $"
        spawn bash -c \"echo 'test foo content' | ($scooter_binary) -s 'foo' -r 'bar' -X\"
        expect eof
    " | complete)

    if $result1.exit_code != 0 {
        print "❌ FAILED: scooter TUI immediate mode with stdin failed to execute"
        print $"Exit code: ($result1.exit_code)"
        print $"Stderr: ($result1.stderr)"
        print $"Output: ($result1.stdout)"
        return 1
    }

    # Check that the replacement was performed correctly in stdout (in test environment)
    if not ($result1.stdout | str contains "test bar content") {
        print "❌ FAILED: scooter TUI immediate mode with stdin produced incorrect output"
        print $"Expected 'test bar content' in stdout"
        print $"Actual stdout: ($result1.stdout)"
        print $"Actual stderr: ($result1.stderr)"
        return 1
    }

    # Test basic TUI mode with stdin - should work with proper terminal
    # Use -X flag to avoid interactive prompts in automated testing
    let test_input = "hello world foo bar\nline two with foo\nline three"
    let result2 = (^expect -c $"
        spawn bash -c \"echo '($test_input)' | ($scooter_binary) -s 'foo' -r 'baz' -X\"
        expect eof
    " | complete)

    if $result2.exit_code != 0 {
        print "❌ FAILED: scooter TUI mode with multi-line stdin failed to execute"
        print $"Exit code: ($result2.exit_code)"
        print $"Stderr: ($result2.stderr)"
        print $"Output: ($result2.stdout)"
        return 1
    }

    # Check that all replacements were performed correctly in stdout (in test environment)
    if not ($result2.stdout | str contains "hello world baz bar") or not ($result2.stdout | str contains "line two with baz") {
        print "❌ FAILED: scooter TUI mode with multi-line stdin produced incorrect output"
        print $"Stdout should contain both 'hello world baz bar' and 'line two with baz'"
        print $"Actual stdout: ($result2.stdout)"
        print $"Actual stderr: ($result2.stderr)"
        return 1
    }

    # Test that original content is preserved where no matches exist
    if not ($result2.stdout | str contains "line three") {
        print "❌ FAILED: scooter TUI mode should preserve non-matching lines"
        print $"Expected 'line three' to be preserved in stdout"
        print $"Actual stdout: ($result2.stdout)"
        return 1
    }

    print "✅ PASSED: TUI mode correctly processes stdin input and produces expected output"
    0
}

def test_stdin_vs_sed_comparison [scooter_binary: string] {
    print "Testing stdin processing vs sed..."

    # Test cases that should match sed behavior
    let test_cases = [
        {input: "hello world", search: "world", replace: "universe", desc: "simple replacement"},
        {input: "foo\nbar\nfoo", search: "foo", replace: "baz", desc: "multi-line replacement"},
        {input: "test123test", search: "test", replace: "exam", desc: "multiple matches on same line"},
        {input: "no matches here", search: "xyz", replace: "abc", desc: "no matches"},
        {input: "", search: "foo", replace: "bar", desc: "empty input"},
    ]

    for case in $test_cases {
        let scooter_result = (do { echo $case.input | ^$scooter_binary --no-tui -s $case.search -r $case.replace } | complete)
        let sed_result = (do { echo $case.input | sed $"s/($case.search)/($case.replace)/g" } | complete)

        if $scooter_result.exit_code != 0 {
            print $"❌ FAILED: scooter failed on ($case.desc)"
            print $"Exit code: ($scooter_result.exit_code)"
            print $"Stderr: ($scooter_result.stderr)"
            return 1
        }

        if $sed_result.exit_code != 0 {
            print $"❌ FAILED: sed failed on ($case.desc)"
            return 1
        }

        if $scooter_result.stdout != $sed_result.stdout {
            print $"❌ FAILED: scooter vs sed mismatch on ($case.desc)"
            print $"Input: ($case.input)"
            print $"scooter: ($scooter_result.stdout)"
            print $"Sed: ($sed_result.stdout)"
            return 1
        }
    }

    print "✅ PASSED: scooter stdin behavior matches sed for basic cases"
    0
}

def test_stdin_edge_cases [scooter_binary: string] {
    print "Testing stdin edge cases..."

    # Test with special characters
    let special_input = "line with\ttabs and\r\nCRLF and\nnormal LF"
    let expected_special = "line with\ttabs plus\r\nCRLF plus\nnormal LF"
    let result1 = (do { echo $special_input | ^$scooter_binary --no-tui -s "and" -r "plus" } | complete)
    if $result1.exit_code != 0 {
        print "❌ FAILED: scooter failed with special characters"
        print $"Exit code: ($result1.exit_code)"
        print $"Stderr: ($result1.stderr)"
        return 1
    }

    # Verify that both "and" occurrences were replaced
    if not ($result1.stdout | str contains "tabs plus") or not ($result1.stdout | str contains "CRLF plus") {
        print "❌ FAILED: scooter did not replace all instances with special characters"
        print $"Expected: ($expected_special)"
        print $"Actual: ($result1.stdout)"
        return 1
    }

    # Test with Unicode characters
    let unicode_input = "café naïve résumé 中文测试 🔥 emoji"
    let expected_unicode = "café simple résumé 中文测试 🔥 emoji"
    let result2 = (do { echo $unicode_input | ^$scooter_binary --no-tui -s "naïve" -r "simple" } | complete)
    if $result2.exit_code != 0 or ($result2.stdout != $expected_unicode) {
        print "❌ FAILED: scooter failed with Unicode characters"
        print $"Exit code: ($result2.exit_code)"
        print $"Stderr: ($result2.stderr)"
        print $"Expected: ($expected_unicode)"
        print $"Actual: ($result2.stdout)"
        return 1
    }

    # Test with large number of lines (stress test)
    let many_lines = (1..100 | each { |i| $"line ($i) with foo content" } | str join "\n")
    let result3 = (do { echo $many_lines | ^$scooter_binary --no-tui -s "foo" -r "bar" } | complete)
    if $result3.exit_code != 0 {
        print "❌ FAILED: scooter failed with many lines"
        print $"Exit code: ($result3.exit_code)"
        print $"Stderr: ($result3.stderr)"
        return 1
    }

    # Check that all lines were processed
    let line_count = ($result3.stdout | lines | length)
    if $line_count != 100 {
        print $"❌ FAILED: scooter processed wrong number of lines (expected 100, got ($line_count))"
        return 1
    }

    # Verify that all "foo" instances were replaced with "bar"
    if ($result3.stdout | str contains "foo") {
        print "❌ FAILED: scooter did not replace all 'foo' instances in many lines test"
        print "Output still contains 'foo'"
        return 1
    }

    if not ($result3.stdout | str contains "bar") {
        print "❌ FAILED: scooter output does not contain expected 'bar' replacements"
        return 1
    }

    print "✅ PASSED: scooter handles stdin edge cases correctly"
    0
}

def test_stdin_validation_errors [scooter_binary: string] {
    print "Testing stdin validation errors..."

    # Test --hidden flag rejected with stdin
    let result1 = (do { echo "test content" | ^$scooter_binary --no-tui -s "foo" -r "bar" --hidden } | complete)
    if $result1.exit_code == 0 or (not ($result1.stderr | str contains "Cannot use --hidden flag when processing stdin")) {
        print "❌ FAILED: scooter should reject --hidden flag with stdin"
        print $"Exit code: ($result1.exit_code)"
        print $"Stderr: ($result1.stderr)"
        return 1
    }

    # Test --files-to-include rejected with stdin
    let result2 = (do { echo "test content" | ^$scooter_binary --no-tui -s "foo" -r "bar" --files-to-include "*.txt" } | complete)
    if $result2.exit_code == 0 or (not ($result2.stderr | str contains "Cannot use --files-to-include when processing stdin")) {
        print "❌ FAILED: scooter should reject --files-to-include flag with stdin"
        print $"Exit code: ($result2.exit_code)"
        print $"Stderr: ($result2.stderr)"
        return 1
    }

    # Test --files-to-exclude rejected with stdin
    let result3 = (do { echo "test content" | ^$scooter_binary --no-tui -s "foo" -r "bar" --files-to-exclude "*.txt" } | complete)
    if $result3.exit_code == 0 or (not ($result3.stderr | str contains "Cannot use --files-to-exclude when processing stdin")) {
        print "❌ FAILED: scooter should reject --files-to-exclude flag with stdin"
        print $"Exit code: ($result3.exit_code)"
        print $"Stderr: ($result3.stderr)"
        return 1
    }

    # Test invalid regex with stdin
    let result4 = (do { echo "test content" | ^$scooter_binary --no-tui -s "(" -r "replacement" } | complete)
    if $result4.exit_code == 0 or (not ($result4.stderr | str contains "Failed to parse search text")) {
        print "❌ FAILED: scooter should reject invalid regex with stdin"
        print $"Exit code: ($result4.exit_code)"
        print $"Stderr: ($result4.stderr)"
        return 1
    }

    print "✅ PASSED: scooter correctly validates stdin input and flags"
    0
}

def main [mode: string, --update-readme, --repo-url: string = ""] {
    let valid_modes = ["test", "benchmark"]
    if $mode not-in $valid_modes {
        print $"❌ ERROR: invalid mode ($mode), must be one of ($valid_modes | str join ', ')"
        exit 1
    }

    let project_dir = $env.PWD
    const replacement_dir = "test-input"
    let scooter_binary = ($project_dir | path join "target" "release" "scooter")

    if not ($scooter_binary | path exists) {
        print $"❌ ERROR: binary not found at ($scooter_binary)"
        exit 1
    }

    let search_term = "before"
    let replace_term = "after"
    let all_tools = get_tools $scooter_binary $search_term $replace_term

    let tool_directories = $all_tools | each {|tool| tool_to_dirname $tool.name}
    let cleanup_dirs = if ($repo_url | is-not-empty) {
        $tool_directories
    } else {
        [$replacement_dir] | append $tool_directories
    }

    try {
        # Ensure nothing is left over from a previous test
        cleanup_directories $cleanup_dirs

        # Run
        let exit_code = if $mode == "benchmark" {
            run_benchmark $project_dir $search_term $replace_term $scooter_binary $update_readme $repo_url
        } else if $mode == "test" {
            let results = [
                (run_e2e_tests $replacement_dir $all_tools $repo_url)
                (test_stdin_processing $scooter_binary)
                (test_stdin_tui_mode $scooter_binary)
                (test_stdin_vs_sed_comparison $scooter_binary)
                (test_stdin_edge_cases $scooter_binary)
                (test_stdin_validation_errors $scooter_binary)
            ]
            if ($results | math sum) == 0 { 0 } else { 1 }
        }

        # Cleanup
        print "Cleaning up test directories..."
        cd $project_dir
        cleanup_directories $cleanup_dirs

        exit $exit_code
    } catch { |err|
        print "Cleaning up after error..."
        cd $project_dir
        cleanup_directories $cleanup_dirs
        print $"❌ TEST FAILED: ($err)"
        exit 1
    }
}
