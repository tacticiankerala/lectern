// Draws from the invented readings beside its page, as an artifact's module script would, and
// tries to read that file's text as well, which the page may not.
import { readings } from "./readings.mjs";

function line(text) {
  const p = document.createElement("p");
  p.textContent = text;
  document.body.append(p);
}

line(`Charted ${readings.length} invented readings.`);
fetch("readings.mjs").then(
  () => line("Read the file beside it."),
  () => line("Couldn't read the file beside it."),
);
