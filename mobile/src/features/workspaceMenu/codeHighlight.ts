/**
 * A small syntax colourer for the phone's code viewer: comments, strings,
 * numbers, keywords and type names, per language family. Pure text in, plain
 * tokens out. It reads a line at a time and carries block comments and
 * multi-line strings across lines, so a file can be coloured page by page.
 */

export type CodeTokenKind = "plain" | "comment" | "string" | "number" | "keyword" | "type";

export interface CodeToken {
  readonly kind: CodeTokenKind;
  readonly text: string;
}

interface Language {
  readonly line: readonly string[];
  readonly block: readonly (readonly [string, string])[];
  /** Quote characters; a string ends on its line unless listed in `multiline`. */
  readonly quotes: readonly string[];
  readonly multiline: readonly string[];
  readonly keywords: ReadonlySet<string>;
  /** Capitalised identifiers are type names. */
  readonly types: boolean;
}

const words = (text: string): ReadonlySet<string> => new Set(text.split(" "));

const C_LIKE = "if else for while do switch case default break continue return try catch finally throw new delete typeof instanceof in of void this super class extends implements interface enum const let var function async await yield import export from as static public private protected readonly abstract override get set true false null undefined type namespace declare satisfies keyof is";
const RUST = "fn let mut const static struct enum impl trait for in while loop if else match return break continue pub crate super self Self use mod as where move ref dyn async await unsafe extern type true false";
const GO = "func var const type struct interface map chan package import return if else for range switch case default break continue go defer select fallthrough nil true false";
const PYTHON = "def class return if elif else for while in not and or is None True False import from as try except finally raise with lambda pass break continue yield async await global nonlocal assert del";
const SWIFT_KOTLIN = "func fun let var val class struct enum protocol extension interface object if else for while repeat switch when case default return break continue guard defer in is as import package try catch throw throws rethrows init deinit self super nil null true false public private internal fileprivate open override static final lazy weak unowned mutating async await data sealed companion suspend";
const SHELL = "if then else elif fi for while until do done case esac function in return exit export local readonly set unset echo cd source";
const SQL = "select from where insert into values update set delete create table alter drop index join left right inner outer on as and or not null primary key foreign references group by order having limit offset union distinct begin commit rollback";

const LANGUAGES: Record<string, Language> = {
  c: { line: ["//"], block: [["/*", "*/"]], quotes: ['"', "'", "`"], multiline: ["`"], keywords: words(C_LIKE), types: true },
  rust: { line: ["//"], block: [["/*", "*/"]], quotes: ['"'], multiline: ['"'], keywords: words(RUST), types: true },
  go: { line: ["//"], block: [["/*", "*/"]], quotes: ['"', "'", "`"], multiline: ["`"], keywords: words(GO), types: true },
  python: { line: ["#"], block: [['"""', '"""'], ["'''", "'''"]], quotes: ['"', "'"], multiline: [], keywords: words(PYTHON), types: true },
  swift: { line: ["//"], block: [["/*", "*/"]], quotes: ['"'], multiline: [], keywords: words(SWIFT_KOTLIN), types: true },
  shell: { line: ["#"], block: [], quotes: ['"', "'"], multiline: ['"', "'"], keywords: words(SHELL), types: false },
  sql: { line: ["--"], block: [["/*", "*/"]], quotes: ["'", '"'], multiline: [], keywords: words(SQL), types: false },
  data: { line: ["#", "//"], block: [["/*", "*/"]], quotes: ['"', "'"], multiline: [], keywords: words("true false null yes no on off"), types: false },
  markup: { line: [], block: [["<!--", "-->"]], quotes: ['"', "'"], multiline: [], keywords: words(""), types: false },
  css: { line: [], block: [["/*", "*/"]], quotes: ['"', "'"], multiline: [], keywords: words("important inherit initial unset none auto"), types: false },
  plain: { line: [], block: [], quotes: [], multiline: [], keywords: words(""), types: false },
};

const BY_EXTENSION: Record<string, string> = {
  ts: "c", tsx: "c", js: "c", jsx: "c", mjs: "c", cjs: "c", java: "c", cs: "c", cpp: "c", cc: "c", h: "c", hpp: "c", m: "c", mm: "c", php: "c", dart: "c", scala: "c",
  rs: "rust", go: "go", py: "python", rb: "python",
  swift: "swift", kt: "swift", kts: "swift",
  sh: "shell", bash: "shell", zsh: "shell", fish: "shell", ps1: "shell",
  sql: "sql",
  json: "data", jsonc: "data", yaml: "data", yml: "data", toml: "data", ini: "data", env: "data", lock: "data",
  html: "markup", htm: "markup", xml: "markup", svg: "markup", vue: "markup", plist: "markup",
  css: "css", scss: "css", less: "css",
};

const BY_NAME: Record<string, string> = { dockerfile: "shell", makefile: "shell", ".gitignore": "shell", ".env": "data" };

/** The language family for a file name; `plain` when unknown. */
export function codeLanguage(fileName: string): string {
  const name = fileName.split("/").pop()?.toLowerCase() ?? "";
  if (BY_NAME[name]) return BY_NAME[name]!;
  const dot = name.lastIndexOf(".");
  return (dot >= 0 ? BY_EXTENSION[name.slice(dot + 1)] : undefined) ?? "plain";
}

/** What an earlier line left open: the text that closes it, and its kind. */
export interface CodeCarry {
  readonly close: string;
  readonly kind: "comment" | "string";
}

const IDENT_START = /[A-Za-z_$]/u;
const IDENT = /[A-Za-z0-9_$]/u;
const NUMBER = /^(?:0[xX][0-9a-fA-F_]+|0[bB][01_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?)/u;

/** Colour one line. Returns its tokens and what stays open for the next line. */
export function highlightLine(
  line: string,
  language: string,
  carry: CodeCarry | null,
): { tokens: CodeToken[]; carry: CodeCarry | null } {
  const lang = LANGUAGES[language] ?? LANGUAGES.plain!;
  const tokens: CodeToken[] = [];
  let plain = "";
  const push = (kind: CodeTokenKind, text: string) => {
    if (text === "") return;
    if (kind === "plain") {
      plain += text;
      return;
    }
    if (plain !== "") {
      tokens.push({ kind: "plain", text: plain });
      plain = "";
    }
    tokens.push({ kind, text });
  };
  let index = 0;
  let open = carry;
  while (index < line.length) {
    if (open) {
      let end = line.indexOf(open.close, index);
      // A backslash before the closing quote keeps the string open.
      while (open.kind === "string" && end > 0 && line[end - 1] === "\\" && line[end - 2] !== "\\") {
        end = line.indexOf(open.close, end + 1);
      }
      if (end < 0) {
        push(open.kind, line.slice(index));
        index = line.length;
        break;
      }
      push(open.kind, line.slice(index, end + open.close.length));
      index = end + open.close.length;
      open = null;
      continue;
    }
    const rest = line.slice(index);
    const block = lang.block.find(([start]) => rest.startsWith(start));
    if (block) {
      push("plain", "");
      const kind = block[0].length === 3 && (block[0] === '"""' || block[0] === "'''") ? "string" : "comment";
      // Emit the opener, then let the loop find its close (maybe on a later line).
      push(kind, block[0]);
      index += block[0].length;
      open = { close: block[1], kind };
      continue;
    }
    if (lang.line.some((start) => rest.startsWith(start))) {
      push("comment", rest);
      index = line.length;
      break;
    }
    const char = line[index]!;
    if (lang.quotes.includes(char)) {
      push("string", char);
      index += 1;
      open = { close: char, kind: "string" };
      // A one-line string that never closes ends with its line.
      if (!lang.multiline.includes(char) && line.indexOf(char, index) < 0) {
        push("string", line.slice(index));
        index = line.length;
        open = null;
      }
      continue;
    }
    if (/\d/u.test(char) && (index === 0 || !IDENT.test(line[index - 1]!))) {
      const match = NUMBER.exec(rest);
      if (match) {
        push("number", match[0]);
        index += match[0].length;
        continue;
      }
    }
    if (IDENT_START.test(char)) {
      let end = index + 1;
      while (end < line.length && IDENT.test(line[end]!)) end += 1;
      const word = line.slice(index, end);
      const keyword = lang.keywords.has(language === "sql" ? word.toLowerCase() : word);
      push(keyword ? "keyword" : lang.types && /^[A-Z][A-Za-z0-9]*[a-z]/u.test(word) ? "type" : "plain", word);
      index = end;
      continue;
    }
    push("plain", char);
    index += 1;
  }
  if (plain !== "") tokens.push({ kind: "plain", text: plain });
  // Only multi-line constructs stay open past the end of the line.
  if (open && open.kind === "string" && !lang.multiline.includes(open.close) && open.close.length === 1) open = null;
  return { tokens, carry: open };
}

/** Colour a whole text; one token list per line. */
export function highlightCode(text: string, language: string): CodeToken[][] {
  const out: CodeToken[][] = [];
  let carry: CodeCarry | null = null;
  for (const line of text.split("\n")) {
    const result = highlightLine(line, language, carry);
    out.push(result.tokens);
    carry = result.carry;
  }
  return out;
}
