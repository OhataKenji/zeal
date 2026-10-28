/** Font metrics shared by the xterm terminal and surrounding CSS. */
export const TERMINAL_FONT = {
  fontFamily: "Menlo, Monaco, monospace",
  fontSize: 13,
  lineHeight: 1,
};

/** ANSI colors tuned for readable text on the terminal's white background. */
export const TERMINAL_COLORS = {
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

/** Complete xterm theme, including the shared ANSI palette. */
export const TERMINAL_THEME = {
  background: "#ffffff",
  foreground: "#222222",
  cursor: "#222222",
  selectionBackground: "#cce0ff",
  ...TERMINAL_COLORS,
};

/** Inline CSS used to make non-xterm console output match terminal metrics. */
export const TERMINAL_TEXT_STYLE = `font-family:${TERMINAL_FONT.fontFamily};font-size:${TERMINAL_FONT.fontSize}px;line-height:normal;color:${TERMINAL_THEME.foreground};background:${TERMINAL_THEME.background};--terminal-selection:${TERMINAL_THEME.selectionBackground}`;
