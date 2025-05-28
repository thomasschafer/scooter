#!/usr/bin/env nu

# Constants
const BASE_DIR = "test-input"

const TEST_CONFIG = {
    search_term: "before",
    replace_term: "after",
    test_files: [
        {path: "file1.txt", content: "This is before text in file1\nAnother line with before\n"},
        {path: "file2.txt", content: "No match here\nJust some text\n"},
        {path: "file3.rs", content: "fn before() {\n    println!(\"before\");\n}\n\n\n"},
        # {path: "subdir/file4.txt", content: "before at start\nMiddle before text\nbefore at end"}, # TODO: fix this - if there is no newline at the end, Scooter adds one
        {path: "subdir/file5.txt", content: "Nothing to replace here"},
    ]
}

def create_test_files [dir: string, files: list] {
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
    return $"output-($name | str replace --all ' + ' '-' | str replace --all ' ' '-')"
}

def run_tool [tool: record, base_dir: string, search_term: string, replace_term: string] {
    let tool_dir = (tool_to_dirname $tool.name)

    print $"\n=== Running ($tool.name) ==="
    cp -r $base_dir $tool_dir

    cd $tool_dir
    ^bash -c $tool.command

    return $tool_dir
}

def compare_directories [dir1: string, dir2: string, name1: string, name2: string] {
    print $"\n=== Comparing ($name1) vs ($name2) ==="

    let diff_result = (^diff -r $dir1 $dir2 | complete)
    let passed = ($diff_result.exit_code == 0)

    if $passed {
        print $"✅ PASSED: ($name1) and ($name2) produced identical results"
    } else {
        print $"❌ FAILED: ($name1) and ($name2) produced different results"
        print "\nDifferences found:"
        print $diff_result.stdout
    }

    return $passed
}

def get_tools [scooter_binary: string] {
    return [
        {
            name: "scooter",
            command: $"script -q /dev/null ($scooter_binary) -X -s ($TEST_CONFIG.search_term) -r ($TEST_CONFIG.replace_term)"
        },
        {
            name: "rg + sd",
            command: $"rg -l ($TEST_CONFIG.search_term) | xargs sd ($TEST_CONFIG.search_term) ($TEST_CONFIG.replace_term)"
        },
    ]
}

def run_all_tools [tools: list, project_dir: string] {
    $tools | each {|tool|
        let tool_dir = run_tool $tool $BASE_DIR $TEST_CONFIG.search_term $TEST_CONFIG.replace_term
        cd $project_dir
        {name: $tool.name, dir: $tool_dir}
    }
}

def compare_results [tool_results: list] {
   let scooter_result = ($tool_results | where name == "scooter" | get dir.0)
    mut all_passed = true

    for result in ($tool_results | where name != "scooter") {
        let passed = compare_directories $scooter_result $result.dir "scooter" $result.name
        if not $passed {
            $all_passed = false
        }
    }

    return $all_passed
}

def main [] {
    print "Running scooter end-to-end tests..."

    let project_dir = $env.PWD
    let scooter_binary = ($project_dir | path join "target" "release" "scooter")

    if not ($scooter_binary | path exists) {
        print $"❌ ERROR: Scooter binary not found at ($scooter_binary)"
        print "Please build scooter first with: cargo build --release"
        exit 1
    }

    let all_tools = (get_tools $scooter_binary)

    let tool_dirs = ($all_tools | each {|tool| tool_to_dirname $tool.name})
    let all_dirs = ([$BASE_DIR] | append $tool_dirs)

    try {
        # Setup
        cleanup_directories $all_dirs
        mkdir $BASE_DIR
        create_test_files $BASE_DIR $TEST_CONFIG.test_files

        # Run tools and compare
        let tool_results = (run_all_tools $all_tools $project_dir)
        let all_passed = (compare_results $tool_results)

        # Cleanup
        print "\nCleaning up test directories..."
        cd $project_dir
        cleanup_directories $all_dirs

        # Report results
        if $all_passed {
            print "\n✅ ALL TESTS PASSED"
            exit 0
        } else {
            print "\n❌ SOME TESTS FAILED"
            exit 1
        }

    } catch { |err|
        print "\nCleaning up after error..."
        cd $project_dir
        cleanup_directories $all_dirs
        print $"❌ TEST FAILED: ($err)"
        exit 1
    }
}
