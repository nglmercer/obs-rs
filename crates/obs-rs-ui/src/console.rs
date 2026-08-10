use std::fmt;

use obs_rs_media::{FrameTransition, MediaError};

use super::{
    types::{UiCommand, UiLocale},
    MAX_CONSOLE_COMMAND_BYTES,
};

/// A command understood by the safe terminal frontend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsoleCommand {
    /// Print the current accessible snapshot.
    Show,
    /// Print command help.
    Help,
    /// Apply one desktop-state command.
    Apply(UiCommand),
    /// End the frontend session.
    Quit,
}

/// Errors raised while parsing terminal frontend commands.
#[derive(Debug, Eq, PartialEq)]
pub enum ConsoleCommandError {
    /// The command contained no non-whitespace text.
    Empty,
    /// The command exceeded [`MAX_CONSOLE_COMMAND_BYTES`].
    TooLong,
    /// The first word is not a supported command.
    UnknownCommand(String),
    /// A command did not contain a required argument.
    MissingArgument(&'static str),
    /// A command contained an invalid subcommand or extra argument.
    InvalidArgument {
        command: &'static str,
        value: String,
    },
    /// A fade transition was outside the valid range.
    InvalidTransition(MediaError),
}

impl fmt::Display for ConsoleCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("console command is empty"),
            Self::TooLong => formatter.write_str("console command is too long"),
            Self::UnknownCommand(command) => write!(formatter, "unknown console command {command}"),
            Self::MissingArgument(argument) => {
                write!(formatter, "missing console argument {argument}")
            }
            Self::InvalidArgument { command, value } => {
                write!(formatter, "invalid argument for {command}: {value}")
            }
            Self::InvalidTransition(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ConsoleCommandError {}

/// Parses one line for the terminal frontend without mutating desktop state.
///
/// # Errors
///
/// Returns [`ConsoleCommandError`] when the line is empty, oversized, unknown,
/// missing an argument, or contains an invalid transition/output action.
pub fn parse_console_command(line: &str) -> Result<ConsoleCommand, ConsoleCommandError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(ConsoleCommandError::Empty);
    }
    if line.len() > MAX_CONSOLE_COMMAND_BYTES {
        return Err(ConsoleCommandError::TooLong);
    }

    let mut words = line.split_whitespace();
    let command = words.next().ok_or(ConsoleCommandError::Empty)?;
    match command {
        "help" => ensure_no_extra("help", words).map(|()| ConsoleCommand::Help),
        "show" => ensure_no_extra("show", words).map(|()| ConsoleCommand::Show),
        "snapshot" => ensure_no_extra("snapshot", words).map(|()| ConsoleCommand::Show),
        "quit" => ensure_no_extra("quit", words).map(|()| ConsoleCommand::Quit),
        "exit" => ensure_no_extra("exit", words).map(|()| ConsoleCommand::Quit),
        "swap" => ensure_no_extra("swap", words)
            .map(|()| ConsoleCommand::Apply(UiCommand::SwapPreviewProgram)),
        "preview" => {
            let id = required_word(&mut words, "preview scene")?;
            ensure_no_extra("preview", words)?;
            Ok(ConsoleCommand::Apply(UiCommand::SelectPreviewScene {
                id: id.to_owned(),
            }))
        }
        "program" => {
            let id = required_word(&mut words, "program scene")?;
            ensure_no_extra("program", words)?;
            Ok(ConsoleCommand::Apply(UiCommand::SelectProgramScene {
                id: id.to_owned(),
            }))
        }
        "profile" => {
            let id = required_word(&mut words, "profile")?;
            ensure_no_extra("profile", words)?;
            Ok(ConsoleCommand::Apply(UiCommand::SelectProfile {
                id: id.to_owned(),
            }))
        }
        "language" => {
            let code = required_word(&mut words, "language")?;
            ensure_no_extra("language", words)?;
            let locale =
                UiLocale::from_code(code).ok_or_else(|| ConsoleCommandError::InvalidArgument {
                    command: "language",
                    value: code.to_owned(),
                })?;
            Ok(ConsoleCommand::Apply(UiCommand::SetLocale { locale }))
        }
        "record" => parse_output_command("record", words, true),
        "stream" => parse_output_command("stream", words, false),
        "mixer" => parse_mixer_command("mixer", words),
        "transition" => parse_transition_command("transition", words),
        "take" => parse_take_command("take", words),
        _ => Err(ConsoleCommandError::UnknownCommand(command.to_owned())),
    }
}

fn required_word<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    argument: &'static str,
) -> Result<&'a str, ConsoleCommandError> {
    words
        .next()
        .ok_or(ConsoleCommandError::MissingArgument(argument))
}

fn ensure_no_extra<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
) -> Result<(), ConsoleCommandError> {
    if let Some(value) = words.next() {
        return Err(ConsoleCommandError::InvalidArgument {
            command,
            value: value.to_owned(),
        });
    }
    Ok(())
}

fn parse_output_command<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
    recording: bool,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    let action = required_word(&mut words, "start or stop")?;
    ensure_no_extra(command, words)?;
    let start = match action {
        "start" => true,
        "stop" => false,
        value => {
            return Err(ConsoleCommandError::InvalidArgument {
                command,
                value: value.to_owned(),
            });
        }
    };
    let command = if recording {
        if start {
            UiCommand::StartRecording
        } else {
            UiCommand::StopRecording
        }
    } else if start {
        UiCommand::StartStreaming
    } else {
        UiCommand::StopStreaming
    };
    Ok(ConsoleCommand::Apply(command))
}

fn parse_transition_command<'a>(
    command: &'static str,
    words: impl Iterator<Item = &'a str>,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    Ok(ConsoleCommand::Apply(UiCommand::SetTransition {
        transition: parse_transition_value(command, words)?,
    }))
}

fn parse_take_command<'a>(
    command: &'static str,
    words: impl Iterator<Item = &'a str>,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    Ok(ConsoleCommand::Apply(UiCommand::TakePreview {
        transition: parse_transition_value(command, words)?,
    }))
}

fn parse_transition_value<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
) -> Result<FrameTransition, ConsoleCommandError> {
    let kind = required_word(&mut words, "cut or fade")?;
    Ok(match kind {
        "cut" => {
            ensure_no_extra(command, words)?;
            FrameTransition::Cut
        }
        "fade" => {
            let progress = required_word(&mut words, "fade progress in 0..1000")?;
            ensure_no_extra(command, words)?;
            let progress =
                progress
                    .parse::<u16>()
                    .map_err(|_| ConsoleCommandError::InvalidArgument {
                        command,
                        value: progress.to_owned(),
                    })?;
            FrameTransition::cross_fade(progress).map_err(ConsoleCommandError::InvalidTransition)?
        }
        value => {
            return Err(ConsoleCommandError::InvalidArgument {
                command,
                value: value.to_owned(),
            });
        }
    })
}

fn parse_mixer_command<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    let id = required_word(&mut words, "mixer channel")?;
    let action = required_word(&mut words, "gain or mute")?;
    let command = match action {
        "mute" => {
            ensure_no_extra(command, words)?;
            UiCommand::ToggleMixerMute { id: id.to_owned() }
        }
        "gain" => {
            let value = required_word(&mut words, "mixer gain 0..2000")?;
            ensure_no_extra(command, words)?;
            let gain_milli =
                value
                    .parse::<u16>()
                    .map_err(|_| ConsoleCommandError::InvalidArgument {
                        command,
                        value: value.to_owned(),
                    })?;
            if gain_milli > 2_000 {
                return Err(ConsoleCommandError::InvalidArgument {
                    command,
                    value: value.to_owned(),
                });
            }
            UiCommand::SetMixerGain {
                id: id.to_owned(),
                gain_milli,
            }
        }
        value => {
            return Err(ConsoleCommandError::InvalidArgument {
                command,
                value: value.to_owned(),
            });
        }
    };
    Ok(ConsoleCommand::Apply(command))
}
