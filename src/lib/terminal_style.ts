/** Font metrics shared by the xterm terminal and surrounding CSS. */
export const TERMINAL_FONT = {
  fontFamily: "Menlo, Monaco, monospace",
  fontSize: 13,
  lineHeight: 1,
};

/** ANSI colors tuned for readable text on the dark terminal background (Equi-luminance & chroma). */
export const TERMINAL_COLORS = {
  black: "#27272a",
  red: "#fb817a",
  green: "#38c789",
  yellow: "#e19b1b",
  blue: "#58b0ff",
  magenta: "#c190f6",
  cyan: "#00c3df",
  white: "#d4d4d8",
  brightBlack: "#52525b",
  brightRed: "#fb9d97",
  brightGreen: "#5fe0a6",
  brightYellow: "#f5b746",
  brightBlue: "#8ac7ff",
  brightMagenta: "#d7b0fb",
  brightCyan: "#45dcf3",
  brightWhite: "#f4f4f5",
};

/** Complete xterm theme, including the shared ANSI palette. */
export const TERMINAL_THEME = {
  background: "#09090b",
  foreground: "#f4f4f5",
  cursor: "#0891b2",
  selectionBackground: "rgba(8, 145, 178, 0.35)",
  ...TERMINAL_COLORS,
};

/** Fixed pixel height of one log line; matches the previously computed height at Menlo 13px. Virtualization depends on it staying fixed. */
export const LOG_LINE_HEIGHT = 15;

/** Inline CSS used to make non-xterm console output match terminal metrics. */
export const TERMINAL_TEXT_STYLE = `font-family:${TERMINAL_FONT.fontFamily};font-size:${TERMINAL_FONT.fontSize}px;line-height:${LOG_LINE_HEIGHT}px;color:${TERMINAL_THEME.foreground};background:${TERMINAL_THEME.background};--terminal-selection:${TERMINAL_THEME.selectionBackground}`;
