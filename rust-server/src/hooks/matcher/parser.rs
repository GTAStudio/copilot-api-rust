use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "hooks/matcher/grammar.pest"]
pub struct MatcherParser;
