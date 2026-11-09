# Persistence by Takeover-on-vanish, not immediate re-offer

On every Selection change the Daemon reads the Offer eagerly (original MIME order, app-private types skipped, 500 ms per type, 2 s and 64 MB per Offer, keeping whatever was read if some types fail), but it only becomes the Selection's owner (**Takeover**) when the Selection goes empty, i.e. when the source app exits. While the source is alive it keeps serving its own data untouched.

wl-clip-persist re-offers immediately on every copy, and that replacement is the root cause of its breakage list (Firefox double `set_selection`, GTK/Emacs paste hangs, IntelliJ cut, Inkscape, GIMP 3, Discord). Mutter avoids it by persisting only one type; we keep all useful types and avoid it by not competing with a live source.

## Consequences

- The Selection must stay empty for 150 ms before the Takeover. Chromium/Electron and GLFW apps copy by destroying their old source before setting the new one, so it blinks empty on every copy but the first; an immediate Takeover there landed after the new copy and silently replaced it with the old Clip.
- An app that *deliberately* clears the clipboard will see the last content restored. Exceptions: Secret and Concealed Clips are never restored, and `hoarder clear` is honoured.
- This is a hypothesis until a prototype is exercised against Firefox, Chromium/Electron, GTK, Qt, IntelliJ and LibreOffice.
