use std::io::Read;
use std::process::ExitCode;

use slides::messages::{MessageCode, Msg, params, render};
use slides::{DEFAULT_MAX_INPUT_BYTES, Error, Options, parse_input, run, schema};

fn failure(error: Error) -> ExitCode {
    println!("{}", serde_json::json!(error));
    ExitCode::FAILURE
}

fn cli_error(code: MessageCode, detail: &dyn std::fmt::Display) -> ExitCode {
    failure(Error::Input(Msg::new(
        code,
        params(&[("detail", &detail.to_string())]),
    )))
}

fn main() -> ExitCode {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(_) => return cli_error(MessageCode::CliArgumentEncoding, &""),
        }
    }
    if args == ["--messages"] || args == ["docs", "--messages"] {
        println!("{}", slides::messages::catalogue());
        return ExitCode::SUCCESS;
    }
    if args.len() > 1 {
        return cli_error(MessageCode::CliArguments, &"");
    }
    if args
        .first()
        .is_some_and(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!("{}", render(MessageCode::CliHelp, &Vec::new(), "en"));
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|arg| arg == "--schema") {
        match serde_json::to_string_pretty(&schema()) {
            Ok(text) => {
                println!("{text}");
                return ExitCode::SUCCESS;
            }
            Err(error) => return cli_error(MessageCode::CliSchemaRender, &error),
        }
    }
    let json = if let Some(json) = args.pop() {
        json
    } else {
        let mut buffer = String::new();
        if let Err(error) = std::io::stdin()
            .take(DEFAULT_MAX_INPUT_BYTES.saturating_add(1))
            .read_to_string(&mut buffer)
        {
            return cli_error(MessageCode::CliStdinRead, &error);
        }
        buffer
    };
    if json.len() as u64 > DEFAULT_MAX_INPUT_BYTES {
        return failure(Error::TooLarge {
            path: "request".into(),
            size: json.len() as u64,
            limit: DEFAULT_MAX_INPUT_BYTES,
            field: String::new(),
        });
    }
    let input = match parse_input(&json) {
        Ok(input) => input,
        Err(error) => return failure(error),
    };
    let base_dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => return cli_error(MessageCode::CliCurrentDir, &error),
    };
    match run(&input, &Options::new(base_dir)) {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(error) => cli_error(MessageCode::CliResultRender, &error),
        },
        Err(error) => failure(error),
    }
}
