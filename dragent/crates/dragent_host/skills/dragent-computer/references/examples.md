# Worked examples

Apps, window ids and element numbers below are illustrations: take yours from
your own `windows` and `snapshot`.

## Filling a form in a native app

```sh
dragent computer windows "Invoices"
# 1  id 5531  “New invoice”  at 220,140 900×700  (focused)
dragent computer snapshot "Invoices" --window-id 5531
#   [3] text field "Customer" value=""
#   [4] text field "Amount" value=""
#   [9] button "Save" actions: press
dragent computer set-value "Invoices" 3 "Acme Ltd"
# … verification: verified (value read back)
dragent computer set-value "Invoices" 4 "1200"
dragent computer click "Invoices" 9
# … verification: unverified (pressed)
dragent computer snapshot "Invoices" --window-id 5531    # saved? an error shown?
```

`set-value` is checked by reading the field back; the Save press isn't, so the
fresh snapshot is the evidence. Saving an invoice is an outside change: have
the person's go-ahead first.

## Using menus

```sh
dragent computer menus "Preview"
# File ▸
#   Open… (⌘O)
#   Export… (⇧⌘E)
#   Export as PDF…
# …
dragent computer menu "Preview" "File > Export as PDF"
# Chose File › Export as PDF… in Preview. … snapshot to see what it did.
dragent computer snapshot "Preview"                       # the save sheet
```

Titles match whatever their case or trailing "…". A menu that opens a submenu
can't be chosen itself (the answer lists its items); a disabled one answers
`element_not_clickable`. On Windows the shortcuts read like `Ctrl+Shift+E`, on
Linux the same way. An app with no readable menu bar answers
`element_not_found`: use the shortcut, with `hotkey`, or its menu button from a
snapshot.

## Arranging windows side by side

```sh
dragent computer windows "TextEdit"
# 1  id 4182  “notes.txt”  at 40,60 600×500
dragent computer window "TextEdit" --window-id 4182 --x 0 --y 25 --width 720 --height 860
# “notes.txt” is at 0, 25, 720×860.  (verified)
dragent computer window "Preview" --x 720 --y 25 --width 720 --height 860
# “report.pdf” is at 720, 25, 720×600: the app didn't take all of it …  (partial)
```

Coordinates are screen points, as `windows` reports frames. `partial` means the
app limited it (a maximum size, kept on screen; on Linux, the window manager's
borders can shift it a little): report the real frame. On Wayland `window`
answers `unsupported_capability`. Don't rearrange the person's windows unless
they asked.

## A stale element

```sh
dragent computer click "Mail" 14
# Take a snapshot of Mail first: element numbers come from it …
dragent computer snapshot "Mail"         # another action (maybe another agent's) changed it
dragent computer click "Mail" 22         # the same button, its new number
```

## A permission missing

```sh
dragent computer snapshot "Notes"
# {"code":"permission_denied", … "Grant Accessibility to Dragent …"}
dragent computer permissions
# accessibility: Missing  screen_recording: Granted
```

Tell the person exactly which permission to allow (System Settings → Privacy &
Security → Accessibility) and wait. Don't open the settings or retry in a loop.
