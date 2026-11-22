/** Interprets input for the preview's neutral simulated process: a terminal
 * that echoes each line back and stays alive until Ctrl-C or Ctrl-D. */
export function interpretConsoleInput(input: string, text: string) {
  let output = "";
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    switch (ch) {
      case "\x03":
        return { input, output: output + "^C\r\n", exitCode: 130 };
      case "\x04":
        return { input, output: output + "^D\r\n", exitCode: 130 };
      case "\r":
      case "\n":
        input = "";
        output += "\r\n";
        if (ch === "\r" && text[i + 1] === "\n") i++;
        break;
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
