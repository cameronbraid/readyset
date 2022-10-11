use std::collections::HashSet;

use lazy_static::lazy_static;
use maplit::hashset;
use nom::branch::alt;
use nom::bytes::complete::{tag, tag_no_case};
use nom::combinator::{map, peek};
use nom::sequence::terminated;
use nom_locate::LocatedSpan;

use crate::common::eof;
use crate::NomSqlResult;

// NOTE: Each keyword_$start_letter_to_$end_letter function uses `alt`,
// which is implemented for tuples sizes up to 21. Because of this constraint
// on maximum tuple sizes, keywords are aggregated into groups of 20

fn keyword_follow_char(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        peek(alt((
            tag(" "),
            tag("\n"),
            tag(";"),
            tag("("),
            tag(")"),
            tag("\t"),
            tag(","),
            tag("="),
            eof,
        ))),
        |i: LocatedSpan<&[u8]>| *i,
    )(i)
}

fn keyword_a_to_c(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        alt((
            terminated(tag_no_case("ABORT"), keyword_follow_char),
            terminated(tag_no_case("ACTION"), keyword_follow_char),
            terminated(tag_no_case("ADD"), keyword_follow_char),
            terminated(tag_no_case("AFTER"), keyword_follow_char),
            terminated(tag_no_case("ALL"), keyword_follow_char),
            terminated(tag_no_case("ALTER"), keyword_follow_char),
            terminated(tag_no_case("ANALYZE"), keyword_follow_char),
            terminated(tag_no_case("AND"), keyword_follow_char),
            terminated(tag_no_case("AS"), keyword_follow_char),
            terminated(tag_no_case("ASC"), keyword_follow_char),
            terminated(tag_no_case("ATTACH"), keyword_follow_char),
            terminated(tag_no_case("AUTOINCREMENT"), keyword_follow_char),
            terminated(tag_no_case("BEFORE"), keyword_follow_char),
            terminated(tag_no_case("BEGIN"), keyword_follow_char),
            terminated(tag_no_case("BETWEEN"), keyword_follow_char),
            terminated(tag_no_case("BY"), keyword_follow_char),
            terminated(tag_no_case("CASCADE"), keyword_follow_char),
            terminated(tag_no_case("CASE"), keyword_follow_char),
            terminated(tag_no_case("CAST"), keyword_follow_char),
            terminated(tag_no_case("CHANGE"), keyword_follow_char),
            terminated(tag_no_case("CHECK"), keyword_follow_char),
        )),
        |i| *i,
    )(i)
}

fn keyword_c_to_e(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        alt((
            terminated(tag_no_case("COLLATE"), keyword_follow_char),
            terminated(tag_no_case("COLUMN"), keyword_follow_char),
            terminated(tag_no_case("COMMIT"), keyword_follow_char),
            terminated(tag_no_case("CONFLICT"), keyword_follow_char),
            terminated(tag_no_case("CONSTRAINT"), keyword_follow_char),
            terminated(tag_no_case("CREATE"), keyword_follow_char),
            terminated(tag_no_case("CROSS"), keyword_follow_char),
            terminated(tag_no_case("DEFERRABLE"), keyword_follow_char),
            terminated(tag_no_case("DEFERRED"), keyword_follow_char),
            terminated(tag_no_case("DELETE"), keyword_follow_char),
            terminated(tag_no_case("DESC"), keyword_follow_char),
            terminated(tag_no_case("DETACH"), keyword_follow_char),
            terminated(tag_no_case("DISTINCT"), keyword_follow_char),
            terminated(tag_no_case("DROP"), keyword_follow_char),
            terminated(tag_no_case("EACH"), keyword_follow_char),
            terminated(tag_no_case("ELSE"), keyword_follow_char),
            terminated(tag_no_case("END"), keyword_follow_char),
            terminated(tag_no_case("ESCAPE"), keyword_follow_char),
            terminated(tag_no_case("EXCEPT"), keyword_follow_char),
            terminated(tag_no_case("EXCLUSIVE"), keyword_follow_char),
            terminated(tag_no_case("EXISTS"), keyword_follow_char),
        )),
        |i| *i,
    )(i)
}

fn keyword_e_to_i(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        alt((
            terminated(tag_no_case("EXPLAIN"), keyword_follow_char),
            terminated(tag_no_case("FAIL"), keyword_follow_char),
            terminated(tag_no_case("FOR"), keyword_follow_char),
            terminated(tag_no_case("FOREIGN"), keyword_follow_char),
            terminated(tag_no_case("FROM"), keyword_follow_char),
            terminated(tag_no_case("FULL"), keyword_follow_char),
            terminated(tag_no_case("FULLTEXT"), keyword_follow_char),
            terminated(tag_no_case("GLOB"), keyword_follow_char),
            terminated(tag_no_case("GROUP"), keyword_follow_char),
            terminated(tag_no_case("GROUPS"), keyword_follow_char),
            terminated(tag_no_case("HAVING"), keyword_follow_char),
            terminated(tag_no_case("ILIKE"), keyword_follow_char),
            terminated(tag_no_case("IGNORE"), keyword_follow_char),
            terminated(tag_no_case("IMMEDIATE"), keyword_follow_char),
            terminated(tag_no_case("IN"), keyword_follow_char),
            terminated(tag_no_case("INDEX"), keyword_follow_char),
            terminated(tag_no_case("INDEXED"), keyword_follow_char),
            terminated(tag_no_case("INITIALLY"), keyword_follow_char),
            terminated(tag_no_case("INNER"), keyword_follow_char),
            terminated(tag_no_case("INSTEAD"), keyword_follow_char),
            terminated(tag_no_case("INTERSECT"), keyword_follow_char),
        )),
        |i| *i,
    )(i)
}

fn keyword_i_to_p(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        alt((
            terminated(tag_no_case("INTO"), keyword_follow_char),
            terminated(tag_no_case("IS"), keyword_follow_char),
            terminated(tag_no_case("JOIN"), keyword_follow_char),
            terminated(tag_no_case("KEY"), keyword_follow_char),
            terminated(tag_no_case("LIKE"), keyword_follow_char),
            terminated(tag_no_case("LIMIT"), keyword_follow_char),
            terminated(tag_no_case("MATCH"), keyword_follow_char),
            terminated(tag_no_case("MODIFY"), keyword_follow_char),
            terminated(tag_no_case("NATURAL"), keyword_follow_char),
            terminated(tag_no_case("NO"), keyword_follow_char),
            terminated(tag_no_case("NOT"), keyword_follow_char),
            terminated(tag_no_case("NOTNULL"), keyword_follow_char),
            terminated(tag_no_case("NULL"), keyword_follow_char),
            terminated(tag_no_case("OF"), keyword_follow_char),
            terminated(tag_no_case("OFFSET"), keyword_follow_char),
            terminated(tag_no_case("ON"), keyword_follow_char),
            terminated(tag_no_case("OR"), keyword_follow_char),
            terminated(tag_no_case("ORDER"), keyword_follow_char),
            terminated(tag_no_case("OUTER"), keyword_follow_char),
            terminated(tag_no_case("PLAN"), keyword_follow_char),
            terminated(tag_no_case("PRAGMA"), keyword_follow_char),
        )),
        |i| *i,
    )(i)
}

fn keyword_p_to_t(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        alt((
            terminated(tag_no_case("PRIMARY"), keyword_follow_char),
            terminated(tag_no_case("QUERY"), keyword_follow_char),
            terminated(tag_no_case("RAISE"), keyword_follow_char),
            terminated(tag_no_case("RECURSIVE"), keyword_follow_char),
            terminated(tag_no_case("REFERENCES"), keyword_follow_char),
            terminated(tag_no_case("REGEXP"), keyword_follow_char),
            terminated(tag_no_case("REINDEX"), keyword_follow_char),
            terminated(tag_no_case("RELEASE"), keyword_follow_char),
            terminated(tag_no_case("RENAME"), keyword_follow_char),
            terminated(tag_no_case("RESTRICT"), keyword_follow_char),
            terminated(tag_no_case("RIGHT"), keyword_follow_char),
            terminated(tag_no_case("ROLLBACK"), keyword_follow_char),
            terminated(tag_no_case("ROW"), keyword_follow_char),
            terminated(tag_no_case("SAVEPOINT"), keyword_follow_char),
            terminated(tag_no_case("SELECT"), keyword_follow_char),
            terminated(tag_no_case("SET"), keyword_follow_char),
            terminated(tag_no_case("TABLE"), keyword_follow_char),
            terminated(tag_no_case("TEMP"), keyword_follow_char),
            terminated(tag_no_case("TEMPORARY"), keyword_follow_char),
            terminated(tag_no_case("THEN"), keyword_follow_char),
            terminated(tag_no_case("TO"), keyword_follow_char),
        )),
        |i| *i,
    )(i)
}

fn keyword_t_to_z(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        alt((
            terminated(tag_no_case("TRANSACTION"), keyword_follow_char),
            terminated(tag_no_case("TRIGGER"), keyword_follow_char),
            terminated(tag_no_case("UNION"), keyword_follow_char),
            terminated(tag_no_case("UNIQUE"), keyword_follow_char),
            terminated(tag_no_case("UPDATE"), keyword_follow_char),
            terminated(tag_no_case("USING"), keyword_follow_char),
            terminated(tag_no_case("VACUUM"), keyword_follow_char),
            terminated(tag_no_case("VIEW"), keyword_follow_char),
            terminated(tag_no_case("VIRTUAL"), keyword_follow_char),
            terminated(tag_no_case("WHEN"), keyword_follow_char),
            terminated(tag_no_case("WHERE"), keyword_follow_char),
            terminated(tag_no_case("WITH"), keyword_follow_char),
            terminated(tag_no_case("WITHOUT"), keyword_follow_char),
        )),
        |i| *i,
    )(i)
}

fn builtin_function_a_to_z(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    map(
        alt((
            terminated(tag_no_case("CURRENT_DATE"), keyword_follow_char),
            terminated(tag_no_case("CURRENT_TIME"), keyword_follow_char),
            terminated(tag_no_case("CURRENT_TIMESTAMP"), keyword_follow_char),
            terminated(tag_no_case("DATABASE"), keyword_follow_char),
            terminated(tag_no_case("DEFAULT"), keyword_follow_char),
            terminated(tag_no_case("IF"), keyword_follow_char),
            terminated(tag_no_case("IN"), keyword_follow_char),
            terminated(tag_no_case("INSERT"), keyword_follow_char),
            terminated(tag_no_case("ISNULL"), keyword_follow_char),
            terminated(tag_no_case("LEFT"), keyword_follow_char),
            terminated(tag_no_case("REPLACE"), keyword_follow_char),
            terminated(tag_no_case("RIGHT"), keyword_follow_char),
            terminated(tag_no_case("VALUES"), keyword_follow_char),
        )),
        |i| *i,
    )(i)
}

// Matches any SQL reserved keyword
pub fn sql_keyword(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    alt((
        keyword_a_to_c,
        keyword_c_to_e,
        keyword_e_to_i,
        keyword_i_to_p,
        keyword_p_to_t,
        keyword_t_to_z,
    ))(i)
}

// Matches any built-in SQL function
pub fn sql_builtin_function(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    builtin_function_a_to_z(i)
}

// Matches any SQL reserved keyword _or_ built-in function
pub fn sql_keyword_or_builtin_function(i: LocatedSpan<&[u8]>) -> NomSqlResult<&[u8], &[u8]> {
    alt((sql_keyword, sql_builtin_function))(i)
}

lazy_static! {
    /// A list of POSGTRES keywords that are not reserved and can be used as
    /// identifiers. For example `CREATE TABLE VARCHAR (id int)` is fine
    /// https://www.postgresql.org/docs/14/sql-keywords-appendix.html
    pub static ref POSTGRES_NOT_RESERVED: HashSet<&'static [u8]> = hashset![
        &b"ABORT"[..],
        b"ABSOLUTE",
        b"ACCESS",
        b"ACTION",
        b"ADD",
        b"ADMIN",
        b"AFTER",
        b"AGGREGATE",
        b"ALSO",
        b"ALTER",
        b"ALWAYS",
        b"ASENSITIVE",
        b"ASSERTION",
        b"ASSIGNMENT",
        b"AT",
        b"ATOMIC",
        b"ATTACH",
        b"ATTRIBUTE",
        b"BACKWARD",
        b"BEFORE",
        b"BEGIN",
        b"BREADTH",
        b"BY",
        b"CACHE",
        b"CALL",
        b"CALLED",
        b"CASCADE",
        b"CASCADED",
        b"CATALOG",
        b"CHAIN",
        b"CHARACTERISTICS",
        b"CHECKPOINT",
        b"CLASS",
        b"CLOSE",
        b"CLUSTER",
        b"COLUMNS",
        b"COMMENT",
        b"COMMENTS",
        b"COMMIT",
        b"COMMITTED",
        b"COMPRESSION",
        b"CONFIGURATION",
        b"CONFLICT",
        b"CONNECTION",
        b"CONSTRAINTS",
        b"CONTENT",
        b"CONTINUE",
        b"CONVERSION",
        b"COPY",
        b"COST",
        b"CSV",
        b"CUBE",
        b"CURRENT",
        b"CURSOR",
        b"CYCLE",
        b"DATA",
        b"DATABASE",
        b"DEALLOCATE",
        b"DECLARE",
        b"DEFAULTS",
        b"DEFERRED",
        b"DEFINER",
        b"DELETE",
        b"DELIMITER",
        b"DELIMITERS",
        b"DEPENDS",
        b"DEPTH",
        b"DETACH",
        b"DICTIONARY",
        b"DISABLE",
        b"DISCARD",
        b"DOCUMENT",
        b"DOMAIN",
        b"DOUBLE",
        b"DROP",
        b"EACH",
        b"ENABLE",
        b"ENCODING",
        b"ENCRYPTED",
        b"ENUM",
        b"ESCAPE",
        b"EVENT",
        b"EXCLUDE",
        b"EXCLUDING",
        b"EXCLUSIVE",
        b"EXECUTE",
        b"EXPLAIN",
        b"EXPRESSION",
        b"EXTENSION",
        b"EXTERNAL",
        b"FAMILY",
        b"FINALIZE",
        b"FIRST",
        b"FOLLOWING",
        b"FORCE",
        b"FORWARD",
        b"FUNCTION",
        b"FUNCTIONS",
        b"GENERATED",
        b"GLOBAL",
        b"GRANTED",
        b"GROUPS",
        b"HANDLER",
        b"HEADER",
        b"HOLD",
        b"IDENTITY",
        b"IF",
        b"IMMEDIATE",
        b"IMMUTABLE",
        b"IMPLICIT",
        b"IMPORT",
        b"INCLUDE",
        b"INCLUDING",
        b"INCREMENT",
        b"INDEX",
        b"INDEXES",
        b"INHERIT",
        b"INHERITS",
        b"INLINE",
        b"INPUT",
        b"INSENSITIVE",
        b"INSERT",
        b"INSTEAD",
        b"INVOKER",
        b"ISOLATION",
        b"KEY",
        b"LABEL",
        b"LANGUAGE",
        b"LARGE",
        b"LAST",
        b"LEAKPROOF",
        b"LEVEL",
        b"LISTEN",
        b"LOAD",
        b"LOCAL",
        b"LOCATION",
        b"LOCK",
        b"LOCKED",
        b"LOGGED",
        b"MAPPING",
        b"MATCH",
        b"MATERIALIZED",
        b"MAXVALUE",
        b"METHOD",
        b"MINVALUE",
        b"MODE",
        b"MOVE",
        b"NAME",
        b"NAMES",
        b"NEW",
        b"NEXT",
        b"NFC",
        b"NFD",
        b"NFKC",
        b"NFKD",
        b"NO",
        b"NORMALIZED",
        b"NOTHING",
        b"NOTIFY",
        b"NOWAIT",
        b"NULLS",
        b"OBJECT",
        b"OF",
        b"OFF",
        b"OIDS",
        b"OLD",
        b"OPERATOR",
        b"OPTION",
        b"OPTIONS",
        b"ORDINALITY",
        b"OTHERS",
        b"OVERRIDING",
        b"OWNED",
        b"OWNER",
        b"PARALLEL",
        b"PARSER",
        b"PARTIAL",
        b"PARTITION",
        b"PASSING",
        b"PASSWORD",
        b"PLANS",
        b"POLICY",
        b"PRECEDING",
        b"PREPARE",
        b"PREPARED",
        b"PRESERVE",
        b"PRIOR",
        b"PRIVILEGES",
        b"PROCEDURAL",
        b"PROCEDURE",
        b"PROCEDURES",
        b"PROGRAM",
        b"PUBLICATION",
        b"QUOTE",
        b"RANGE",
        b"READ",
        b"REASSIGN",
        b"RECHECK",
        b"RECURSIVE",
        b"REF",
        b"REFERENCING",
        b"REFRESH",
        b"REINDEX",
        b"RELATIVE",
        b"RELEASE",
        b"RENAME",
        b"REPEATABLE",
        b"REPLACE",
        b"REPLICA",
        b"RESET",
        b"RESTART",
        b"RESTRICT",
        b"RETURN",
        b"RETURNS",
        b"REVOKE",
        b"ROLE",
        b"ROLLBACK",
        b"ROLLUP",
        b"ROUTINE",
        b"ROUTINES",
        b"ROWS",
        b"RULE",
        b"SAVEPOINT",
        b"SCHEMA",
        b"SCHEMAS",
        b"SCROLL",
        b"SEARCH",
        b"SECURITY",
        b"SEQUENCE",
        b"SEQUENCES",
        b"SERIALIZABLE",
        b"SERVER",
        b"SESSION",
        b"SET",
        b"SETS",
        b"SHARE",
        b"SHOW",
        b"SIMPLE",
        b"SKIP",
        b"SNAPSHOT",
        b"SQL",
        b"STABLE",
        b"STANDALONE",
        b"START",
        b"STATEMENT",
        b"STATISTICS",
        b"STDIN",
        b"STDOUT",
        b"STORAGE",
        b"STORED",
        b"STRICT",
        b"STRIP",
        b"SUBSCRIPTION",
        b"SUPPORT",
        b"SYSID",
        b"SYSTEM",
        b"TABLES",
        b"TABLESPACE",
        b"TEMP",
        b"TEMPLATE",
        b"TEMPORARY",
        b"TEXT",
        b"TIES",
        b"TRANSACTION",
        b"TRANSFORM",
        b"TRIGGER",
        b"TRUNCATE",
        b"TRUSTED",
        b"TYPE",
        b"TYPES",
        b"UESCAPE",
        b"UNBOUNDED",
        b"UNCOMMITTED",
        b"UNENCRYPTED",
        b"UNKNOWN",
        b"UNLISTEN",
        b"UNLOGGED",
        b"UNTIL",
        b"UPDATE",
        b"VACUUM",
        b"VALID",
        b"VALIDATE",
        b"VALIDATOR",
        b"VALUE",
        b"VERSION",
        b"VIEW",
        b"VIEWS",
        b"VOLATILE",
        b"WHITESPACE",
        b"WORK",
        b"WRAPPER",
        b"WRITE",
        b"XML",
        b"YES",
        b"ZONE",
        b"BETWEEN",
        b"BIGINT",
        b"BIT",
        b"BOOLEAN",
        b"COALESCE",
        b"DEC",
        b"DECIMAL",
        b"EXISTS",
        b"EXTRACT",
        b"FLOAT",
        b"GREATEST",
        b"GROUPING",
        b"INOUT",
        b"INT",
        b"INTEGER",
        b"INTERVAL",
        b"LEAST",
        b"NATIONAL",
        b"NCHAR",
        b"NONE",
        b"NORMALIZE",
        b"NULLIF",
        b"NUMERIC",
        b"OUT",
        b"OVERLAY",
        b"POSITION",
        b"REAL",
        b"ROW",
        b"SETOF",
        b"SMALLINT",
        b"SUBSTRING",
        b"TIME",
        b"TIMESTAMP",
        b"TREAT",
        b"TRIM",
        b"VALUES",
        b"VARCHAR",
        b"XMLATTRIBUTES",
        b"XMLCONCAT",
        b"XMLELEMENT",
        b"XMLEXISTS",
        b"XMLFOREST",
        b"XMLNAMESPACES",
        b"XMLPARSE",
        b"XMLPI",
        b"XMLROOT",
        b"XMLSERIALIZE",
        b"XMLTABLE",
        b"CHAR",
        b"CHARACTER",
        b"PRECISION",
        b"DAY",
        b"FILTER",
        b"HOUR",
        b"MINUTE",
        b"MONTH",
        b"OVER",
        b"SECOND",
        b"VARYING",
        b"WITHIN",
        b"WITHOUT",
        b"YEAR",
    ];
}
