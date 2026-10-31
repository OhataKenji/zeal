/** Interprets input for the preview's sample process, which doubles one number. */
export function interpretConsoleInput(input: string, text: string) {
  let output = "";
  for (const ch of text) {
    switch (ch) {
      case "\x03":
      case "\x04":
        return {
          input,
          output: output + "^" + (ch === "\x03" ? "C" : "D") + "\r\n",
          exitCode: 130,
        };
      case "\r":
      case "\n":
        return { input, output: output + `\r\n${Number(input) * 2}\r\n`, exitCode: 0 };
      case "\x7f":
        input = input.slice(0, -1);
        output += "\b \b";
        break;
      default:
        input += ch;
        output += ch;
    }
  }
  return { input, output, exitCode: null };
}
