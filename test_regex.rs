use regex::Regex;

fn main() {
    let content = r#"I'll launch Alacritty for you.
[TOOL_OUTPUT]
<function="run_command">
<parameter>command</parameter>
</parameter>
<parameter=arguments">
alacritty
</parameter>
</function>
[/TOOL_OUTPUT]
Alacritty has been launched in the background."#;

    // We can just use standard text slicing if regex is annoying
    println!("Done");
}
