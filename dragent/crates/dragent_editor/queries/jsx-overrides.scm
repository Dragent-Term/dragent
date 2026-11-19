; Appended after the tsx grammar's own highlights query (later patterns win).
;
; JSX text is element content — prose — not a string literal, so it renders in
; the default foreground like other editors, not the string color the stock
; query gives it. `@embedded` is the capture this editor draws as plain text;
; `@none` would not work here, because arborium prefers a styled capture over an
; unstyled one on the same node, so the stock `@string` would still win.
(jsx_text) @embedded
