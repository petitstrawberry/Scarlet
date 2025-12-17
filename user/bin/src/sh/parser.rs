//! Command parser with tokenization and pipeline support
//!
//! This module provides a comprehensive parser for shell commands that supports:
//! - Tokenization with proper quote handling
//! - Pipe operators (|)
//! - I/O redirection (>, >>, <)
//! - Environment variable expansion

#![allow(dead_code)]

extern crate scarlet_std as std;

use std::{string::String, vec::Vec};

/// Types of I/O redirection
#[derive(Debug, Clone, PartialEq)]
pub enum RedirectType {
    Output,  // >
    Append,  // >>
    Input,   // <
}

/// A single command with arguments and redirections
#[derive(Debug, Clone)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub redirects: Vec<(RedirectType, String)>,
}

impl Command {
    pub fn new(program: String) -> Self {
        Self {
            program,
            args: Vec::new(),
            redirects: Vec::new(),
        }
    }
}

/// A pipeline of commands connected by pipes
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub commands: Vec<Command>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn add_command(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// Parse errors
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    EmptyInput,
    UnexpectedToken(String),
    UnclosedQuote,
    InvalidRedirect,
    EmptyPipeline,
}

/// Token types
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Pipe,              // |
    RedirectOut,       // >
    RedirectAppend,    // >>
    RedirectIn,        // <
}

/// Tokenize an input string into tokens
fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut current_word = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // Quote handling
            '"' | '\'' => {
                if in_quotes && c == quote_char {
                    // End of quoted string
                    in_quotes = false;
                } else if !in_quotes {
                    // Start of quoted string
                    in_quotes = true;
                    quote_char = c;
                } else {
                    // Different quote inside quotes - treat as literal
                    current_word.push(c);
                }
            }

            // Whitespace - word separator
            ' ' | '\t' => {
                if in_quotes {
                    current_word.push(c);
                } else if !current_word.is_empty() {
                    tokens.push(Token::Word(current_word.clone()));
                    current_word.clear();
                }
            }

            // Pipe operator
            '|' => {
                if in_quotes {
                    current_word.push(c);
                } else {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }
                    tokens.push(Token::Pipe);
                }
            }

            // Redirect operators
            '>' => {
                if in_quotes {
                    current_word.push(c);
                } else {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }

                    // Check for >>
                    if chars.peek() == Some(&'>') {
                        chars.next();
                        tokens.push(Token::RedirectAppend);
                    } else {
                        tokens.push(Token::RedirectOut);
                    }
                }
            }

            '<' => {
                if in_quotes {
                    current_word.push(c);
                } else {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }
                    tokens.push(Token::RedirectIn);
                }
            }

            // Regular character
            _ => {
                current_word.push(c);
            }
        }
    }

    // Check for unclosed quotes
    if in_quotes {
        return Err(ParseError::UnclosedQuote);
    }

    // Add final word if any
    if !current_word.is_empty() {
        tokens.push(Token::Word(current_word));
    }

    if tokens.is_empty() {
        return Err(ParseError::EmptyInput);
    }

    Ok(tokens)
}

/// Parse tokens into a pipeline
fn parse_tokens(tokens: &[Token]) -> Result<Pipeline, ParseError> {
    let mut pipeline = Pipeline::new();
    let mut current_command: Option<Command> = None;
    let mut expect_redirect_target = false;
    let mut redirect_type: Option<RedirectType> = None;

    for token in tokens {
        match token {
            Token::Word(word) => {
                if expect_redirect_target {
                    // This word is the target of a redirect
                    if let Some(cmd) = current_command.as_mut() {
                        if let Some(rtype) = redirect_type.take() {
                            cmd.redirects.push((rtype, word.clone()));
                        }
                    }
                    expect_redirect_target = false;
                } else if let Some(cmd) = current_command.as_mut() {
                    // Add argument to current command
                    cmd.args.push(word.clone());
                } else {
                    // Start a new command
                    let mut cmd = Command::new(word.clone());
                    cmd.args.push(word.clone()); // First arg is program name
                    current_command = Some(cmd);
                }
            }

            Token::Pipe => {
                if expect_redirect_target {
                    return Err(ParseError::UnexpectedToken(String::from("|")));
                }

                // End current command and add to pipeline
                if let Some(cmd) = current_command.take() {
                    if cmd.args.is_empty() {
                        return Err(ParseError::EmptyPipeline);
                    }
                    pipeline.add_command(cmd);
                } else {
                    return Err(ParseError::UnexpectedToken(String::from("|")));
                }
            }

            Token::RedirectOut => {
                if expect_redirect_target {
                    return Err(ParseError::UnexpectedToken(String::from(">")));
                }
                expect_redirect_target = true;
                redirect_type = Some(RedirectType::Output);
            }

            Token::RedirectAppend => {
                if expect_redirect_target {
                    return Err(ParseError::UnexpectedToken(String::from(">>")));
                }
                expect_redirect_target = true;
                redirect_type = Some(RedirectType::Append);
            }

            Token::RedirectIn => {
                if expect_redirect_target {
                    return Err(ParseError::UnexpectedToken(String::from("<")));
                }
                expect_redirect_target = true;
                redirect_type = Some(RedirectType::Input);
            }
        }
    }

    // Check for incomplete redirect
    if expect_redirect_target {
        return Err(ParseError::InvalidRedirect);
    }

    // Add final command
    if let Some(cmd) = current_command {
        if cmd.args.is_empty() {
            return Err(ParseError::EmptyPipeline);
        }
        pipeline.add_command(cmd);
    }

    if pipeline.is_empty() {
        return Err(ParseError::EmptyPipeline);
    }

    Ok(pipeline)
}

/// Parse a command line into a pipeline
pub fn parse_pipeline(input: &str) -> Result<Pipeline, ParseError> {
    // Expand environment variables
    let expanded = expand_variables(input);

    // Tokenize
    let tokens = tokenize(&expanded)?;

    // Parse tokens into pipeline
    parse_tokens(&tokens)
}

/// Expand environment variables in a string
/// Supports $VAR, ${VAR}, and special variables like $?, $$, $0
fn expand_variables(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            // Check if this is a variable expansion
            if let Some(&next_char) = chars.peek() {
                if next_char == '{' {
                    // Handle ${VAR} syntax
                    chars.next(); // consume '{'
                    let mut var_name = String::new();
                    let mut found_close = false;

                    for var_char in chars.by_ref() {
                        if var_char == '}' {
                            found_close = true;
                            break;
                        }
                        var_name.push(var_char);
                    }

                    if found_close && !var_name.is_empty() {
                        // Expand the variable
                        if let Some(value) = get_variable_value(&var_name) {
                            result.push_str(&value);
                        }
                    } else {
                        // Malformed ${...}, treat as literal
                        result.push('$');
                        result.push('{');
                        result.push_str(&var_name);
                    }
                } else if next_char.is_alphabetic()
                    || next_char == '_'
                    || next_char == '?'
                    || next_char == '$'
                    || next_char == '0'
                {
                    // Handle $VAR syntax and special variables
                    let mut var_name = String::new();

                    if next_char == '?' || next_char == '$' || next_char == '0' {
                        // Special single-character variables
                        var_name.push(chars.next().unwrap());
                    } else {
                        // Regular variable name
                        while let Some(&var_char) = chars.peek() {
                            if var_char.is_alphanumeric() || var_char == '_' {
                                var_name.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }

                    if !var_name.is_empty() {
                        // Expand the variable
                        if let Some(value) = get_variable_value(&var_name) {
                            result.push_str(&value);
                        }
                    } else {
                        result.push('$');
                    }
                } else {
                    // Not a variable, just a literal $
                    result.push('$');
                }
            } else {
                // $ at end of string
                result.push('$');
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Get the value of a variable (environment variable or special variable)
fn get_variable_value(var_name: &str) -> Option<String> {
    match var_name {
        "?" => {
            // Exit status of last command (simplified, always return 0 for now)
            Some(String::from("0"))
        }
        "$" => {
            // Process ID (simplified, return a placeholder)
            Some(String::from("1000"))
        }
        "0" => {
            // Name of the shell or script
            Some(String::from("sh"))
        }
        _ => {
            // Regular environment variable
            std::env::var(var_name)
        }
    }
}
