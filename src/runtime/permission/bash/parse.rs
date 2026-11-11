//! Bounded Bash AST analysis via `brush-parser` (ZCode `bash-command-parser` counterpart).

use super::types::{
    BashAstAnalysis, BashCommandInvocation, BashCommandOperator, BashEnvAssignment, BashRedirect,
};
use brush_parser::ast::{
    AndOr, AssignmentName, AssignmentValue, Command, CommandPrefixOrSuffixItem, CompoundCommand,
    CompoundList, IoFileRedirectKind, IoFileRedirectTarget, IoRedirect, Pipeline, PipelineTimed,
    Program, SeparatorOperator, SimpleCommand, Word,
};
use brush_parser::word::{self, WordPiece, WordPieceWithSource};
use brush_parser::{Parser, ParserOptions, unquote_str};
use std::io::Cursor;

pub const MAX_BASH_PARSE_LENGTH: usize = 10_000;
const SUPPORTED_CONTAINER_NODES: &[&str] = &["AndOr", "Pipeline", "Statement", "CompoundList"];

#[must_use]
pub fn analyze_bash_command(command: &str) -> BashAstAnalysis {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return BashAstAnalysis::empty();
    }
    if command.len() > MAX_BASH_PARSE_LENGTH {
        let mut analysis = BashAstAnalysis::empty();
        analysis.has_parse_errors = true;
        return analysis;
    }
    let options = ParserOptions::default();
    let mut parser = Parser::new(Cursor::new(command.as_bytes()), &options);
    let program = match parser.parse_program() {
        Ok(program) => program,
        Err(_) => {
            let mut analysis = BashAstAnalysis::empty();
            analysis.has_parse_errors = true;
            return analysis;
        }
    };
    let mut analysis = MutableAnalysis::default();
    collect_program(command, &program, &options, &mut analysis);
    analysis.freeze()
}

#[derive(Default)]
struct MutableAnalysis {
    commands: Vec<BashCommandInvocation>,
    has_dynamic_words: bool,
    has_redirects: bool,
    unsupported: Vec<String>,
}

impl MutableAnalysis {
    fn note_unsupported(&mut self, node: &str) {
        if !self.unsupported.iter().any(|e| e == node) {
            self.unsupported.push(node.to_owned());
        }
    }
    fn freeze(self) -> BashAstAnalysis {
        let unsupported_node_types: Vec<String> = self
            .unsupported
            .into_iter()
            .filter(|n| !SUPPORTED_CONTAINER_NODES.contains(&n.as_str()))
            .collect();
        BashAstAnalysis {
            commands: self.commands,
            has_dynamic_words: self.has_dynamic_words,
            has_parse_errors: false,
            has_redirects: self.has_redirects,
            has_unsupported_syntax: !unsupported_node_types.is_empty(),
            unsupported_node_types,
        }
    }
}

fn collect_program(source: &str, program: &Program, options: &ParserOptions, analysis: &mut MutableAnalysis) {
    for (index, complete) in program.complete_commands.iter().enumerate() {
        let op = if index == 0 { None } else { Some(BashCommandOperator::Sequence) };
        collect_compound_list(source, complete, options, analysis, op, &[]);
    }
}

fn collect_compound_list(
    source: &str,
    list: &CompoundList,
    options: &ParserOptions,
    analysis: &mut MutableAnalysis,
    operator_before: Option<BashCommandOperator>,
    inherited: &[BashRedirect],
) {
    for (index, item) in list.0.iter().enumerate() {
        let op = if index == 0 { operator_before } else { Some(BashCommandOperator::Sequence) };
        if matches!(item.1, SeparatorOperator::Async) {
            analysis.note_unsupported("background");
        }
        collect_and_or(source, &item.0, options, analysis, op, inherited);
    }
}

fn collect_and_or(
    source: &str,
    list: &brush_parser::ast::AndOrList,
    options: &ParserOptions,
    analysis: &mut MutableAnalysis,
    operator_before: Option<BashCommandOperator>,
    inherited: &[BashRedirect],
) {
    collect_pipeline(source, &list.first, options, analysis, operator_before, inherited);
    for and_or in &list.additional {
        let (op, pipeline) = match and_or {
            AndOr::And(p) => (BashCommandOperator::And, p),
            AndOr::Or(p) => (BashCommandOperator::Or, p),
        };
        collect_pipeline(source, pipeline, options, analysis, Some(op), inherited);
    }
}

fn collect_pipeline(
    source: &str,
    pipeline: &Pipeline,
    options: &ParserOptions,
    analysis: &mut MutableAnalysis,
    operator_before: Option<BashCommandOperator>,
    inherited: &[BashRedirect],
) {
    let first_index = analysis.commands.len();
    for (index, command) in pipeline.seq.iter().enumerate() {
        let op = if index == 0 { operator_before } else { Some(BashCommandOperator::Pipe) };
        collect_command(source, command, options, analysis, op, inherited);
    }
    if pipeline.timed.is_some() {
        if let Some(first) = analysis.commands.get_mut(first_index) {
            if first.argv.first().map(String::as_str) != Some("time") {
                first.argv.insert(0, "time".to_owned());
                if first.name.is_empty() {
                    first.name = "time".to_owned();
                }
            }
            let _ = matches!(pipeline.timed, Some(PipelineTimed::TimedWithPosixOutput(_)));
        }
    }
}

fn collect_command(
    source: &str,
    command: &Command,
    options: &ParserOptions,
    analysis: &mut MutableAnalysis,
    operator_before: Option<BashCommandOperator>,
    inherited: &[BashRedirect],
) {
    match command {
        Command::Simple(simple) => collect_simple(source, simple, options, analysis, operator_before, inherited),
        Command::Compound(compound, redirects) => {
            if redirects.is_some() {
                analysis.has_redirects = true;
                analysis.note_unsupported("compound_redirect");
            }
            let name = match compound {
                CompoundCommand::Subshell(_) => "Subshell",
                CompoundCommand::BraceGroup(_) => "BraceGroup",
                CompoundCommand::ForClause(_) => "ForClause",
                CompoundCommand::ArithmeticForClause(_) => "ArithmeticForClause",
                CompoundCommand::CaseClause(_) => "CaseClause",
                CompoundCommand::IfClause(_) => "IfClause",
                CompoundCommand::WhileClause(_) => "WhileClause",
                CompoundCommand::UntilClause(_) => "UntilClause",
                CompoundCommand::Arithmetic(_) => "Arithmetic",
            };
            analysis.note_unsupported(name);
        }
        Command::Function(_) => analysis.note_unsupported("FunctionDefinition"),
        Command::ExtendedTest(_, redirects) => {
            if redirects.is_some() {
                analysis.has_redirects = true;
            }
            analysis.note_unsupported("ExtendedTest");
        }
    }
}

fn collect_simple(
    source: &str,
    command: &SimpleCommand,
    options: &ParserOptions,
    analysis: &mut MutableAnalysis,
    operator_before: Option<BashCommandOperator>,
    inherited: &[BashRedirect],
) {
    let mut argv = Vec::new();
    let mut env_assignments = Vec::new();
    let mut redirects = inherited.to_vec();
    let mut has_dynamic_words = false;

    if let Some(prefix) = &command.prefix {
        for item in &prefix.0 {
            match item {
                CommandPrefixOrSuffixItem::AssignmentWord(assignment, _) => {
                    let (value, value_dynamic) = match &assignment.value {
                        AssignmentValue::Scalar(w) => {
                            let dynm = word_is_dynamic(&w.value, options);
                            has_dynamic_words |= dynm;
                            (Some(unquote_str(&w.value)), dynm)
                        }
                        AssignmentValue::Array(_) => {
                            has_dynamic_words = true;
                            (None, true)
                        }
                    };
                    let name = match &assignment.name {
                        AssignmentName::VariableName(n) => Some(n.clone()),
                        AssignmentName::ArrayElementName(_, _) => {
                            has_dynamic_words = true;
                            None
                        }
                    };
                    env_assignments.push(BashEnvAssignment { name, value, value_dynamic });
                }
                CommandPrefixOrSuffixItem::Word(word) => {
                    push_word(&mut argv, &mut has_dynamic_words, word, options);
                }
                CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                    analysis.has_redirects = true;
                    let parsed = convert_redirect(redirect, options);
                    has_dynamic_words |= parsed.dynamic;
                    redirects.push(parsed);
                }
                CommandPrefixOrSuffixItem::ProcessSubstitution(_, _) => {
                    has_dynamic_words = true;
                    analysis.note_unsupported("ProcessSubstitution");
                }
            }
        }
    }
    if let Some(name_word) = &command.word_or_name {
        push_word(&mut argv, &mut has_dynamic_words, name_word, options);
    }
    if let Some(suffix) = &command.suffix {
        for item in &suffix.0 {
            match item {
                CommandPrefixOrSuffixItem::Word(word) => {
                    push_word(&mut argv, &mut has_dynamic_words, word, options);
                }
                CommandPrefixOrSuffixItem::AssignmentWord(_, word) => {
                    has_dynamic_words |= word_is_dynamic(&word.value, options);
                    analysis.note_unsupported("suffix_assignment");
                }
                CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                    analysis.has_redirects = true;
                    let parsed = convert_redirect(redirect, options);
                    has_dynamic_words |= parsed.dynamic;
                    redirects.push(parsed);
                }
                CommandPrefixOrSuffixItem::ProcessSubstitution(_, _) => {
                    has_dynamic_words = true;
                    analysis.note_unsupported("ProcessSubstitution");
                }
            }
        }
    }
    if has_dynamic_words {
        analysis.has_dynamic_words = true;
    }
    if !redirects.is_empty() {
        analysis.has_redirects = true;
    }
    let name = argv.first().cloned().unwrap_or_default();
    let _ = source;
    analysis.commands.push(BashCommandInvocation {
        command_text: command.to_string(),
        argv,
        env_assignments,
        has_dynamic_words,
        has_redirects: !redirects.is_empty(),
        name,
        operator_before,
        redirects,
    });
}

fn push_word(argv: &mut Vec<String>, has_dynamic: &mut bool, word: &Word, options: &ParserOptions) {
    if word_is_dynamic(&word.value, options) || word_has_unquoted_glob(&word.value, options) {
        *has_dynamic = true;
    }
    argv.push(unquote_str(&word.value));
}

fn convert_redirect(redirect: &IoRedirect, options: &ParserOptions) -> BashRedirect {
    match redirect {
        IoRedirect::File(fd, kind, target) => {
            let (target_text, dynamic) = match target {
                IoFileRedirectTarget::Filename(w) | IoFileRedirectTarget::Duplicate(w) => {
                    (unquote_str(&w.value), word_is_dynamic(&w.value, options))
                }
                IoFileRedirectTarget::Fd(n) => (n.to_string(), false),
                IoFileRedirectTarget::ProcessSubstitution(_, _) => (String::new(), true),
            };
            BashRedirect {
                file_descriptor: *fd,
                operator: match kind {
                    IoFileRedirectKind::Read => "<",
                    IoFileRedirectKind::Write => ">",
                    IoFileRedirectKind::Append => ">>",
                    IoFileRedirectKind::ReadAndWrite => "<>",
                    IoFileRedirectKind::Clobber => ">|",
                    IoFileRedirectKind::DuplicateInput => "<&",
                    IoFileRedirectKind::DuplicateOutput => ">&",
                }
                .to_owned(),
                target: target_text,
                dynamic,
            }
        }
        IoRedirect::HereDocument(fd, here) => BashRedirect {
            file_descriptor: *fd,
            operator: if here.remove_tabs { "<<-".into() } else { "<<".into() },
            target: unquote_str(&here.here_end.value),
            dynamic: true,
        },
        IoRedirect::HereString(fd, word) => BashRedirect {
            file_descriptor: *fd,
            operator: "<<<".into(),
            target: unquote_str(&word.value),
            dynamic: word_is_dynamic(&word.value, options),
        },
        IoRedirect::OutputAndError(word, append) => BashRedirect {
            file_descriptor: None,
            operator: if *append { "&>>".into() } else { "&>".into() },
            target: unquote_str(&word.value),
            dynamic: word_is_dynamic(&word.value, options),
        },
    }
}

fn word_is_dynamic(raw: &str, options: &ParserOptions) -> bool {
    match word::parse(raw, options) {
        Ok(pieces) => pieces_dynamic(&pieces) || brace_in_text(&pieces),
        Err(_) => true,
    }
}

fn pieces_dynamic(pieces: &[WordPieceWithSource]) -> bool {
    pieces.iter().any(|p| piece_dynamic(&p.piece))
}

fn piece_dynamic(piece: &WordPiece) -> bool {
    match piece {
        WordPiece::Text(_) | WordPiece::SingleQuotedText(_) | WordPiece::AnsiCQuotedText(_) | WordPiece::EscapeSequence(_) => false,
        WordPiece::DoubleQuotedSequence(inner) | WordPiece::GettextDoubleQuotedSequence(inner) => {
            inner.iter().any(|x| piece_dynamic(&x.piece))
        }
        WordPiece::TildeExpansion(_)
        | WordPiece::ParameterExpansion(_)
        | WordPiece::CommandSubstitution(_)
        | WordPiece::BackquotedCommandSubstitution(_)
        | WordPiece::ArithmeticExpression(_) => true,
    }
}

fn brace_in_text(pieces: &[WordPieceWithSource]) -> bool {
    pieces.iter().any(|p| match &p.piece {
        WordPiece::Text(text) => looks_brace(text),
        WordPiece::DoubleQuotedSequence(inner) | WordPiece::GettextDoubleQuotedSequence(inner) => {
            brace_in_text(inner)
        }
        _ => false,
    })
}

fn looks_brace(text: &str) -> bool {
    let Some(start) = text.find('{') else { return false; };
    let Some(end) = text[start + 1..].find('}') else { return false; };
    let inner = &text[start + 1..start + 1 + end];
    inner.contains(',') || inner.contains("..")
}

fn word_has_unquoted_glob(raw: &str, options: &ParserOptions) -> bool {
    match word::parse(raw, options) {
        Ok(pieces) => pieces.iter().any(|p| matches!(&p.piece, WordPiece::Text(t) if t.chars().any(|c| matches!(c, '*' | '?' | '[')))),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pipeline_and_dynamic() {
        let a = analyze_bash_command("ls | cat");
        assert_eq!(a.commands.len(), 2);
        let b = analyze_bash_command("ls $(touch x)");
        assert!(b.has_dynamic_words);
    }
}
