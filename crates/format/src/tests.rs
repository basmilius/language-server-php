use std::fs;
use std::path::{Path, PathBuf};

use expect_test::{Expect, expect};

use crate::{BraceStyle, Edit, FormatOptions, Indent, Refusal, apply, format, on_type_edits, range_edits, try_format};

fn check(input: &str, expected: Expect) {
    check_with(input, &FormatOptions::default(), expected);
}

fn check_with(input: &str, options: &FormatOptions, expected: Expect) {
    let once = format(input, options).expect("the text is formatted");
    expected.assert_eq(&once);
    let twice = format(&once, options).expect("the result is formatted");
    assert_eq!(once, twice, "formatting is idempotent");
}

#[test]
fn lays_out_a_class_the_way_per_does() {
    check(
        "<?php\ndeclare(strict_types=1);\nnamespace App;\nuse A\\B;use C\\D;\nfinal class User extends Model implements JsonSerializable{\n  private int $a=1;\n  public function __construct(private readonly string $name,protected ?int $age=null){\n     parent::__construct();\n  }\npublic function name():string{\nreturn $this->name;\n}\n}\n",
        expect![[r#"
            <?php
            declare(strict_types=1);

            namespace App;

            use A\B;
            use C\D;

            final class User extends Model implements JsonSerializable
            {
                private int $a = 1;

                public function __construct(private readonly string $name, protected ?int $age = null)
                {
                    parent::__construct();
                }

                public function name(): string
                {
                    return $this->name;
                }
            }
        "#]],
    );
}

#[test]
fn indents_statements_chains_and_nested_brackets() {
    check(
        "<?php\nfunction f(){\n$x=$this->a()\n->b()\n  ->c();\nif($x>1){echo \"a\";}elseif($x){echo 'b';}else{\nfoo(1,2,[1,2,3],function($a)use($b){return $a+$b;});\n}\nforeach($items as $k=>$v){\n$r[$k]=[\n'a'=>1,\n'bcd'=>[\n2,\n],\n];\n}\nswitch($x){\ncase 1:\necho 1;\nbreak;\ndefault:\necho 2;\n}\nreturn match($x){\n1=>'a',\ndefault=>'b',\n};\n}\n",
        expect![[r#"
            <?php
            function f()
            {
                $x = $this->a()
                    ->b()
                    ->c();
                if ($x > 1) {
                    echo "a";
                } elseif ($x) {
                    echo 'b';
                } else {
                    foo(1, 2, [1, 2, 3], function ($a) use ($b) { return $a + $b; });
                }
                foreach ($items as $k => $v) {
                    $r[$k] = [
                        'a' => 1,
                        'bcd' => [
                            2,
                        ],
                    ];
                }
                switch ($x) {
                    case 1:
                        echo 1;
                        break;
                    default:
                        echo 2;
                }
                return match ($x) {
                    1 => 'a',
                    default => 'b',
                };
            }
        "#]],
    );
}

#[test]
fn moves_a_block_comment_along_with_the_code_it_sits_over() {
    check(
        "<?php\nclass A\n{\n\t/**\n\t * Doc\n\t   * odd\n\t */\n\tpublic function a() { // trailing\n\t\t// only comment\n\t}\n        /* block\n           text */\n        public function b() {}\n    // before close\n}\n// end\n",
        expect![[r#"
            <?php
            class A
            {
                /**
                 * Doc
                   * odd
                 */
                public function a()
                { // trailing
                    // only comment
                }

                /* block
                   text */
                public function b() {}
                // before close
            }
            // end
        "#]],
    );
}

#[test]
fn leaves_strings_and_heredocs_alone() {
    check(
        "<?php\n$s=<<<EOT\n    hello {$a}\n      indented\n    EOT;\n$t=\"x {$a['b']} y\".'z';\nfoo(<<<'RAW'\n  raw   text\nRAW,1);\n",
        expect![[r#"
            <?php
            $s = <<<EOT
                hello {$a}
                  indented
                EOT;
            $t = "x {$a['b']} y" . 'z';
            foo(<<<'RAW'
              raw   text
            RAW, 1);
        "#]],
    );
}

#[test]
fn handles_alternative_syntax() {
    check(
        "<?php\nforeach($arr as $k=>$v):\necho $k;\nendforeach;\nif($a):\necho 1;\nelseif($b):\necho 2;\nelse:\necho 3;\nendif;\n",
        expect![[r#"
            <?php
            foreach ($arr as $k => $v):
                echo $k;
            endforeach;
            if ($a):
                echo 1;
            elseif ($b):
                echo 2;
            else:
                echo 3;
            endif;
        "#]],
    );
}

#[test]
fn keeps_the_blank_lines_a_class_body_has_and_drops_them_in_functions() {
    check(
        "<?php\nclass A\n{\n\n    public $a;\n\n\n    public $b;\n\n    public function f()\n    {\n\n        return 1;\n\n    }\n\n}\n",
        expect![[r#"
            <?php
            class A
            {

                public $a;

                public $b;

                public function f()
                {
                    return 1;
                }

            }
        "#]],
    );
}

#[test]
fn separates_top_level_declarations_and_header_blocks() {
    check(
        "<?php\ndeclare(strict_types=1);\nnamespace App;\nuse A\\B;\nclass One {}\nfunction two() {}\ninterface Three {}\n",
        expect![[r#"
            <?php
            declare(strict_types=1);

            namespace App;

            use A\B;

            class One {}

            function two() {}

            interface Three {}
        "#]],
    );
}

#[test]
fn puts_the_brace_and_parenthesis_of_a_split_parameter_list_together() {
    check(
        "<?php\nclass A\n{\n    public function __construct(\n        private int $a,\n        private int $b\n    )\n    {\n    }\n}\n",
        expect![[r#"
            <?php
            class A
            {
                public function __construct(
                    private int $a,
                    private int $b
                )
                {
                }
            }
        "#]],
    );
}

#[test]
fn follows_the_options() {
    let options = FormatOptions {
        indent: Indent::Tab,
        class_brace: BraceStyle::SameLine,
        function_brace: BraceStyle::SameLine,
        blank_lines_between_members: 2,
        ..FormatOptions::default()
    };
    check_with(
        "<?php\nclass A\n{\n    public function a()\n    {\n        return 1;\n    }\n    public function b()\n    {\n        return 2;\n    }\n}\n",
        &options,
        expect![[r#"
            <?php
            class A
            {
            	public function a()
            	{
            		return 1;
            	}


            	public function b()
            	{
            		return 2;
            	}
            }
        "#]],
    );
}

#[test]
fn aligns_arrows_and_assignments_when_asked() {
    let options = FormatOptions {
        align_assignments: true,
        align_array_arrows: true,
        ..FormatOptions::default()
    };
    check_with(
        "<?php\n$a = 1;\n$bcd = 2;\n$ef = [\n    'a' => 1,\n    'bbbb' => 2,\n    'cc' => [\n        'x' => 1,\n        'yyy' => 2,\n    ],\n];\n$z = 3;\n",
        &options,
        expect![[r#"
            <?php
            $a   = 1;
            $bcd = 2;
            $ef  = [
                'a'    => 1,
                'bbbb' => 2,
                'cc'   => [
                    'x'   => 1,
                    'yyy' => 2,
                ],
            ];
            $z = 3;
        "#]],
    );
}

#[test]
fn breaks_a_long_argument_list_and_nested_ones_in_turn() {
    let options = FormatOptions {
        line_length: 60,
        ..FormatOptions::default()
    };
    check_with(
        "<?php\n$result = $service->process($first, $second, $this->other($third, $fourth, $fifth, $sixth), $last);\nfunction a(int $one, string $two, array $three, ?object $four = null): void {}\n",
        &options,
        expect![[r#"
            <?php
            $result = $service->process(
                $first,
                $second,
                $this->other($third, $fourth, $fifth, $sixth),
                $last
            );

            function a(
                int $one,
                string $two,
                array $three,
                ?object $four = null
            ): void {}
        "#]],
    );
}

#[test]
fn keeps_the_line_endings_the_file_has() {
    let formatted = format(
        "<?php\r\nclass A{\r\npublic function f(){\r\nreturn 1;\r\n}\r\n}",
        &FormatOptions::default(),
    );
    assert_eq!(
        formatted.as_deref(),
        Some("<?php\r\nclass A\r\n{\r\n    public function f()\r\n    {\r\n        return 1;\r\n    }\r\n}\r\n")
    );
}

#[test]
fn leaves_what_it_cannot_lay_out() {
    let options = FormatOptions::default();
    assert_eq!(try_format("<?php\nfoo(", &options), Err(Refusal::SyntaxErrors));
    assert_eq!(
        try_format("<div><?php echo 1; ?></div>", &options),
        Err(Refusal::Markup)
    );
    assert_eq!(try_format("  \n", &options), Err(Refusal::Empty));
    assert_eq!(try_format("<?php", &options).as_deref(), Ok("<?php"));
}

#[test]
fn range_formatting_touches_only_the_lines_asked_for() {
    let text = "<?php\nfunction a(){\n$x=1;\n$y=2;\n}\n";
    let start = text.find("$x").expect("found");
    let end = start + 4;
    let edits = range_edits(text, start, end, &FormatOptions::default()).expect("edits");
    assert_eq!(apply(text, &edits), "<?php\nfunction a(){\n    $x = 1;\n$y=2;\n}\n");
}

#[test]
fn on_type_formatting_indents_the_line_it_ends() {
    let options = FormatOptions::default();
    let text = "<?php\nfunction a()\n{\nif ($x) {\n$y = 1;\n}\n}\n";
    let offset = text.find("}\n}").expect("found") + 1;
    let edits = on_type_edits(text, offset, '}', &options).expect("edits");
    assert_eq!(
        apply(text, &edits),
        "<?php\nfunction a()\n{\nif ($x) {\n$y = 1;\n    }\n}\n"
    );

    let offset = text.find("$y = 1;").expect("found") + "$y = 1;".len();
    let edits = on_type_edits(text, offset, ';', &options).expect("edits");
    assert_eq!(
        apply(text, &edits),
        "<?php\nfunction a()\n{\nif ($x) {\n        $y = 1;\n}\n}\n"
    );
}

#[test]
fn on_type_formatting_indents_a_new_line_in_text_that_is_not_finished() {
    let options = FormatOptions::default();
    for (text, expected) in [
        ("<?php\nclass A\n{\n\n", "    "),
        (
            "<?php\nclass A\n{\n    public function f()\n    {\n\n    }\n}\n",
            "        ",
        ),
        ("<?php\nfoo(\n\n", "    "),
        ("<?php\nif ($a) {\n    foo();\n\n}\n", "    "),
        ("<?php\nfoo();\n\n", ""),
    ] {
        let cursor = text.find("\n\n").expect("a blank line") + 1;
        let edits = on_type_edits(text, cursor, '\n', &options).expect("edits");
        let applied = apply(text, &edits);
        let line = applied[cursor..].lines().next().unwrap_or_default();
        assert_eq!(line, expected, "{text:?}");
    }
}

#[test]
fn edits_do_not_overlap() {
    let text = "<?php\nclass A{public function f(){return[1,2];}}\n";
    let edits: Vec<Edit> = crate::edits(text, &FormatOptions::default()).expect("edits");
    for pair in edits.windows(2) {
        assert!(pair[0].end <= pair[1].start);
    }
}

fn corpus_files(name: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !path.ends_with(".git") {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|extension| extension == "php") {
                out.push(path);
            }
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus").join(name);
    let mut files = Vec::new();
    walk(&dir, &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!("skipped: {name} is not fetched (run scripts/fetch-corpus.sh)");
    }
    files
}

#[test]
fn the_stubs_keep_their_tokens_and_format_the_same_twice() {
    let option_sets = [
        FormatOptions::default(),
        FormatOptions {
            indent: Indent::Tab,
            line_length: 60,
            align_assignments: true,
            align_array_arrows: true,
            ..FormatOptions::default()
        },
    ];
    for (files_seen, path) in corpus_files("phpstorm-stubs").into_iter().enumerate() {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, options) in option_sets.iter().enumerate() {
            if index > 0 && files_seen % 8 != 0 {
                continue;
            }
            match try_format(&text, options) {
                Ok(once) => assert_eq!(
                    try_format(&once, options).as_deref(),
                    Ok(once.as_str()),
                    "not idempotent: {}",
                    path.display()
                ),
                Err(Refusal::ChangedTokens) => panic!("tokens would change: {}", path.display()),
                Err(_) => {}
            }
        }
    }
}

#[test]
fn typing_anywhere_in_a_file_never_panics() {
    let text = "<?php\nnamespace A;\n\nclass B extends C\n{\n    public function f(int $a = 1, ...$rest): ?string\n    {\n        $x = [1, 'é' => fn($y) => $y + 1];\n        return match ($a) { 1 => 'ü', default => \"{$x['a']}\" };\n    }\n}\n";
    let options = FormatOptions::default();
    for offset in 0..=text.len() + 1 {
        for typed in ['}', ';', '\n'] {
            let _ = on_type_edits(text, offset, typed, &options);
        }
        let _ = range_edits(text, offset, text.len(), &options);
        let _ = range_edits(text, 0, offset, &options);
    }
    for end in (0..text.len()).filter(|end| text.is_char_boundary(*end)) {
        let _ = on_type_edits(&text[..end], end, '\n', &options);
        let _ = try_format(&text[..end], &options);
    }
}

fn wrapped(width: usize) -> FormatOptions {
    FormatOptions {
        line_length: width,
        ..FormatOptions::default()
    }
}

#[test]
fn a_long_concatenation_breaks_before_its_operators() {
    check_with(
        "<?php\n$message = 'The quick brown fox' . $this->describe($animal) . ' jumps over the lazy dog ' . $another . ' and runs';\n",
        &wrapped(80),
        expect![[r#"
            <?php
            $message = 'The quick brown fox'
                . $this->describe($animal)
                . ' jumps over the lazy dog '
                . $another
                . ' and runs';
        "#]],
    );
}

#[test]
fn a_long_condition_goes_on_lines_of_its_own() {
    check_with(
        "<?php\nif ($this->isEnabled($feature) && $user->hasPermission('some-long-permission-name') || $request->isSecure()) {\n    run();\n}\n",
        &wrapped(80),
        expect![[r#"
            <?php
            if (
                $this->isEnabled($feature)
                && $user->hasPermission('some-long-permission-name')
                || $request->isSecure()
            ) {
                run();
            }
        "#]],
    );
}

#[test]
fn a_long_method_chain_breaks_before_its_arrows() {
    check_with(
        "<?php\n$query = $this->repository->createQueryBuilder('alias')->where('alias.name = :name')->setParameter('name', $name)->getQuery();\n",
        &wrapped(80),
        expect![[r#"
            <?php
            $query = $this->repository
                ->createQueryBuilder('alias')
                ->where('alias.name = :name')
                ->setParameter('name', $name)
                ->getQuery();
        "#]],
    );
}

#[test]
fn a_chain_of_one_call_breaks_its_arguments_instead() {
    check_with(
        "<?php\nreturn $this->service->doSomething($argumentNumberOne, ['key' => 'value', 'other' => $thing], $argumentThree);\n",
        &wrapped(80),
        expect![[r#"
            <?php
            return $this->service->doSomething(
                $argumentNumberOne,
                ['key' => 'value', 'other' => $thing],
                $argumentThree
            );
        "#]],
    );
}

#[test]
fn a_long_array_has_an_item_on_each_line() {
    check_with(
        "<?php\n$items = ['first' => 'value of the first item', 'second' => 'value of the second item', 'third' => 'value'];\n",
        &wrapped(80),
        expect![[r#"
            <?php
            $items = [
                'first' => 'value of the first item',
                'second' => 'value of the second item',
                'third' => 'value'
            ];
        "#]],
    );
}

#[test]
fn a_long_ternary_breaks_before_its_question_mark_and_colon() {
    check_with(
        "<?php\n$value = $condition ? $this->computeTheFirstValue($argumentOne, $argumentTwo) : $this->computeTheSecondValue($argumentOne);\n",
        &wrapped(80),
        expect![[r#"
            <?php
            $value = $condition
                ? $this->computeTheFirstValue($argumentOne, $argumentTwo)
                : $this->computeTheSecondValue($argumentOne);
        "#]],
    );
}

#[test]
fn a_short_ternary_keeps_its_question_mark_and_colon_together() {
    check_with(
        "<?php\n$value = $this->computeTheFirstValue($argumentOne, $argumentTwo, $argumentThree) ?: $this->computeTheSecond($argumentOne);\n",
        &wrapped(80),
        expect![[r#"
            <?php
            $value = $this->computeTheFirstValue($argumentOne, $argumentTwo, $argumentThree)
                ?: $this->computeTheSecond($argumentOne);
        "#]],
    );
}

#[test]
fn nothing_is_broken_that_fits_or_when_wrapping_is_off() {
    let text = "<?php\n$message = 'The quick brown fox' . $this->describe($animal) . ' jumps over the lazy dog ' . $another;\n";
    check_with(
        text,
        &wrapped(0),
        expect![[r#"
        <?php
        $message = 'The quick brown fox' . $this->describe($animal) . ' jumps over the lazy dog ' . $another;
    "#]],
    );
    check_with(
        text,
        &wrapped(200),
        expect![[r#"
        <?php
        $message = 'The quick brown fox' . $this->describe($animal) . ' jumps over the lazy dog ' . $another;
    "#]],
    );
}

#[test]
fn wrapping_changes_no_token_of_a_comment_laden_chain() {
    let text = "<?php\n$x = $this->first() /* one */ ->second($argumentNumberOne, $argumentNumberTwo) // two\n    ->third($argumentNumberThree);\n";
    let formatted = format(text, &wrapped(40)).expect("formatted");
    assert!(crate::same_tokens(text, &formatted));
    assert_eq!(format(&formatted, &wrapped(40)).as_deref(), Some(formatted.as_str()));
}
