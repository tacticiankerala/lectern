// Draws from the invented readings beside its page, as an artifact's module script would, and
// tries to read the data file beside it as well, which the page may not.
import { readings } from "./readings.mjs";

function line(text) {
  const p = document.createElement("p");
  p.textContent = text;
  document.body.append(p);
}

line(`Charted ${readings.length} invented readings.`);
fetch("tides.json").then(
  () => line("Read the tide log beside it."),
  () => line("Couldn't read the tide log beside it."),
);
