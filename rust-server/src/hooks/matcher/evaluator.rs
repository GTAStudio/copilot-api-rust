use pest::iterators::Pair;
use pest::Parser;
use regex::Regex;

use crate::hooks::types::HookInput;

use super::parser::MatcherParser;
use super::parser::Rule;

pub fn evaluate(expr: &str, input: &HookInput) -> Result<bool, String> {
    if expr.trim() == "*" {
        return Ok(true);
    }
    let mut pairs = MatcherParser::parse(Rule::expr, expr).map_err(|e| e.to_string())?;
    let pair = pairs.next().ok_or_else(|| "Empty expression".to_string())?;
    Ok(eval_pair(pair, input))
}

fn eval_pair(pair: Pair<Rule>, input: &HookInput) -> bool {
    match pair.as_rule() {
        Rule::expr | Rule::or_expr => {
            let mut inner = pair.into_inner();
            let mut result = eval_pair(inner.next().unwrap(), input);
            while let Some(op) = inner.next() {
                let rhs = eval_pair(inner.next().unwrap(), input);
                if op.as_str() == "||" {
                    result = result || rhs;
                }
            }
            result
        }
        Rule::and_expr => {
            let mut inner = pair.into_inner();
            let mut result = eval_pair(inner.next().unwrap(), input);
            while let Some(op) = inner.next() {
                let rhs = eval_pair(inner.next().unwrap(), input);
                if op.as_str() == "&&" {
                    result = result && rhs;
                }
            }
            result
        }
        Rule::not_expr => {
            let mut inner = pair.into_inner();
            let first = inner.next().unwrap();
            if first.as_rule() == Rule::neg {
                let value = eval_pair(inner.next().unwrap(), input);
                !value
            } else {
                eval_pair(first, input)
            }
        }
        Rule::primary => {
            let inner = pair.into_inner().next().unwrap();
            eval_pair(inner, input)
        }
        Rule::predicate => {
            let mut inner = pair.into_inner();
            let first = inner.next().unwrap();
            if first.as_str() == "*" {
                return true;
            }
            let field = first.as_str();
            let op = inner.next().unwrap().as_str();
            let value = inner.next().unwrap();
            let rhs = parse_string(value.as_str());
            let lhs = resolve_field(input, field);
            match op {
                "==" => lhs.map(|v| v == rhs).unwrap_or(false),
                "!=" => lhs.map(|v| v != rhs).unwrap_or(false),
                "matches" => {
                    let Ok(re) = Regex::new(&rhs) else { return false; };
                    lhs.map(|v| re.is_match(&v)).unwrap_or(false)
                }
                _ => false,
            }
        }
        Rule::field => {
            resolve_field(input, pair.as_str()).is_some()
        }
        _ => false,
    }
}

fn parse_string(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let inner = &trimmed[1..trimmed.len() - 1];
        inner.replace("\\\"", "\"")
    } else {
        trimmed.to_string()
    }
}

fn resolve_field(input: &HookInput, field: &str) -> Option<String> {
    if field == "tool" {
        return input.tool.clone();
    }
    if field.starts_with("tool_input.") {
        let path = &field["tool_input.".len()..];
        return resolve_json_path(input.tool_input.as_ref(), path);
    }
    if field.starts_with("tool_output.") {
        let path = &field["tool_output.".len()..];
        return resolve_json_path(input.tool_output.as_ref(), path);
    }
    None
}

fn resolve_json_path(value: Option<&serde_json::Value>, path: &str) -> Option<String> {
    let mut current = value?;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    match current {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
