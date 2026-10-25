export const terminalFont = {
  fontFamily: "Menlo, Monaco, monospace",
  fontSize: 13,
  lineHeight: 1,
};

// ANSI hues derive from the timeline trace palette (timeline.ts eventColors —
// orange, green, teal, purple), darkened for text on white. The palette has no
// red or blue, so errors take brick orange and blue borrows indigo.
export const terminalColors = {
  black: "#222222",
  red: "#ac3e15",
  green: "#307e3d",
  yellow: "#967008",
  blue: "#5c58bb",
  magenta: "#8049a2",
  cyan: "#257e6a",
  white: "#d4d4d4",
  brightBlack: "#666666",
  brightRed: "#ca4616",
  brightGreen: "#359742",
  brightYellow: "#ab8007",
  brightBlue: "#6e6bc7",
  brightMagenta: "#9558bb",
  brightCyan: "#2c967d",
  brightWhite: "#eeeeee",
};

export const terminalTheme = {
  background: "#ffffff",
  foreground: "#222222",
  cursor: "#222222",
  selectionBackground: "#cce0ff",
  ...terminalColors,
};

// CSS normal line height uses the same font metrics as xterm's lineHeight: 1.
export const terminalTextStyle = `font-family:${terminalFont.fontFamily};font-size:${terminalFont.fontSize}px;line-height:normal;color:${terminalTheme.foreground};background:${terminalTheme.background};--terminal-selection:${terminalTheme.selectionBackground}`;
