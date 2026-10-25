export const terminalFont = {
  fontFamily: "Menlo, Monaco, monospace",
  fontSize: 13,
  lineHeight: 1,
};

export const terminalColors = {
  black: "#222222",
  red: "#b42318",
  green: "#18713c",
  yellow: "#856400",
  blue: "#175cd3",
  magenta: "#9333a8",
  cyan: "#087c8c",
  white: "#d4d4d4",
  brightBlack: "#666666",
  brightRed: "#d92d20",
  brightGreen: "#218445",
  brightYellow: "#997000",
  brightBlue: "#2563eb",
  brightMagenta: "#ad3dc3",
  brightCyan: "#078899",
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
