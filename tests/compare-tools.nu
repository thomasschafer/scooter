#!/usr/bin/env nu

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
        # TODO: uncomment
        # {
        #     name: "scooter (no tui)",
        #     command: $"($scooter_binary) -N -s '($search_term)' -r '($replace_term)'",
        # },
        {
            name: "rg + sd",
            command: $"rg -l ($search_term) | xargs sd '($search_term)' '($replace_term)'",
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
            run_e2e_tests $replacement_dir $all_tools $repo_url
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
