// Generates fixtures/vault/work/alpha/plans/2026-01-01-big-plan.md, a synthetic implementation
// plan of 3,000+ lines for render tests and benchmarks. The output is deterministic: no randomness,
// no clock. Run with `node fixtures/gen-big-plan.mjs` and commit the result.
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(here, "vault/work/alpha/plans/2026-01-01-big-plan.md");

const SECTIONS = 15;
const TASKS_PER_SECTION = 3;
const DONE_SECTIONS = 2; // steps in the first sections are ticked, so the plan shows progress

const topics = ["widgets", "billing", "search", "exports", "alerts", "reports", "imports"];
const nouns = ["widget", "invoice", "query", "export", "alert", "report", "import"];
const verbs = ["Add", "Validate", "Index", "Render", "Archive", "Schedule", "Migrate"];
const headers = ["Name", "Path", "Owner", "Status", "Notes"];
const owners = ["platform", "billing", "search", "growth"];
const statuses = ["todo", "in review", "done", "blocked"];

const camel = (s) => s[0].toUpperCase() + s.slice(1);
const indentAll = (lines, indent) => lines.map((l) => (l === "" ? "" : indent + l));

function fence(lang, body, indent = "") {
  return indentAll(["```" + lang, ...body, "```"], indent);
}

function rubySpec(noun, id) {
  const klass = `${camel(noun)}${id.replace(".", "")}`;
  return [
    `RSpec.describe ${klass} do`,
    `  let(:record) { described_class.new(name: "${noun}-${id}") }`,
    "",
    `  it "keeps its name" do`,
    `    expect(record.name).to eq("${noun}-${id}")`,
    "  end",
    "",
    `  it "is invalid without a name" do`,
    "    expect(described_class.new(name: nil)).not_to be_valid",
    "  end",
    "end",
  ];
}

function rubyImpl(noun, id) {
  const klass = `${camel(noun)}${id.replace(".", "")}`;
  return [
    `class ${klass} < ApplicationRecord`,
    "  validates :name, presence: true",
    "",
    "  def to_param",
    `    "#{id}-#{name.parameterize}"`,
    "  end",
    "end",
  ];
}

function jsxImpl(noun, id) {
  const comp = `${camel(noun)}Panel${id.replace(".", "")}`;
  return [
    `export function ${comp}({ items, onSelect }) {`,
    "  if (items.length === 0) {",
    `    return <p className="empty">No ${noun}s yet.</p>;`,
    "  }",
    "  return (",
    `    <ul className="${noun}-panel">`,
    "      {items.map((item) => (",
    "        <li key={item.id} onClick={() => onSelect(item)}>",
    "          {item.label}",
    "        </li>",
    "      ))}",
    "    </ul>",
    "  );",
    "}",
  ];
}

function vimImpl(noun, id) {
  const num = id.replace(".", "");
  return [
    `" ${camel(noun)} ${id}: jump between the model and its spec`,
    `nnoremap <leader>${noun[0]}${num} :call ${camel(noun)}Toggle()<CR>`,
    "",
    `function! ${camel(noun)}Toggle() abort`,
    "  let l:file = expand('%:t:r')",
    "  if l:file =~# '_spec$'",
    `    execute 'edit app/models/' . substitute(l:file, '_spec$', '', '') . '.rb'`,
    "  else",
    `    execute 'edit spec/models/' . l:file . '_spec.rb'`,
    "  endif",
    "endfunction",
  ];
}

function table(s, t, noun) {
  const cols = 2 + ((s + t) % 4); // 2 to 5 columns
  const head = headers.slice(0, cols);
  const lines = [`| ${head.join(" | ")} |`, `|${head.map(() => " --- ").join("|")}|`];
  for (let r = 1; r <= 3; r++) {
    const cells = [
      `${noun} ${s}.${t}.${r}`,
      `\`app/models/${noun}_${s}_${t}.rb:${10 * r}\``,
      owners[(s + r) % owners.length],
      statuses[(s + t + r) % statuses.length],
      r === 2 ? "**Needs a migration**" : "Covered by the spec",
    ];
    lines.push(`| ${cells.slice(0, cols).join(" | ")} |`);
  }
  return lines;
}

function task(s, t) {
  const i = (s * TASKS_PER_SECTION + t) % nouns.length;
  const noun = nouns[i];
  const id = `${s}.${t}`;
  const file = `${noun}_${s}_${t}`;
  const box = s <= DONE_SECTIONS ? "[x]" : "[ ]";
  const implLang = ["ruby", "jsx", "vim"][(s + t) % 3];
  const impl =
    implLang === "ruby"
      ? rubyImpl(noun, id)
      : implLang === "jsx"
        ? jsxImpl(noun, id)
        : vimImpl(noun, id);

  return [
    `### Task ${id}: ${verbs[i]} the ${noun} model`,
    "",
    "**Files:**",
    `- Create: \`app/models/${file}.rb\``,
    `- Modify: \`app/controllers/${noun}s_controller.rb:${20 + t}\``,
    `- Test: \`spec/models/${file}_spec.rb\``,
    "",
    `- ${box} **Step 1: Write the failing test**`,
    "",
    ...fence("ruby", rubySpec(noun, id), "  "),
    "",
    `- ${box} **Step 2: Run the test to see it fail**`,
    "",
    ...fence("bash", [`bundle exec rspec spec/models/${file}_spec.rb`], "  "),
    "",
    `  Expected: FAIL with \`NameError: uninitialized constant ${camel(noun)}${s}${t}\`.`,
    "",
    `- ${box} **Step 3: Implement the ${noun}**`,
    "",
    ...fence(implLang, impl, "  "),
    "",
    `- ${box} **Step 4: Run the test again**`,
    "",
    "  Expected: PASS, with no pending examples.",
    "",
    `- ${box} **Step 5: Commit**`,
    "",
    ...fence("bash", [
      `git add app/models/${file}.rb spec/models/${file}_spec.rb`,
      `git commit -m "feat(${topics[s % topics.length]}): ${verbs[i].toLowerCase()} ${noun} ${id}"`,
    ]),
    "",
    ...table(s, t, noun),
    "",
    `> **Note:** keep \`config/${topics[s % topics.length]}.yml\` in sync with this task.`,
    "> The reviewer checks it before merging.",
    "",
  ];
}

function section(s) {
  const topic = topics[s % topics.length];
  const lines = [
    `## Section ${s}`,
    "",
    `Section ${s} covers the ${topic} area. Work through the tasks in order; each one leaves the`,
    `suite green. Read \`docs/${topic}/overview.md\` first, then \`docs/${topic}/decisions.md\`.`,
    "",
  ];
  for (let t = 1; t <= TASKS_PER_SECTION; t++) {
    lines.push(...task(s, t));
  }
  lines.push("---", "");
  return lines;
}

const lines = [
  "---",
  "name: big-plan",
  "status: active",
  "---",
  "",
  "# Big Plan",
  "",
  "> **For agents:** work through the tasks in order and tick each step as it lands.",
  "",
  "**Goal:** a synthetic plan that exercises every construct a long implementation plan uses.",
  "",
  "**Tech stack:** Ruby, React (JSX), Bash and Vim script.",
  "",
  "---",
  "",
];
for (let s = 1; s <= SECTIONS; s++) {
  lines.push(...section(s));
}
lines.push("## Wrap-up", "", "Every section is done when its last task is merged.");

mkdirSync(path.dirname(out), { recursive: true });
writeFileSync(out, lines.join("\n") + "\n");
console.log(`wrote ${lines.length} lines to ${path.relative(process.cwd(), out)}`);
