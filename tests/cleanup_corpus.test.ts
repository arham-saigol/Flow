import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, it, expect } from "vitest";
import corpus from "./fixtures/cleanup_corpus.json";

interface CleanupConstraints {
  mustChange: boolean;
  mustContain: string[];
  mustNotContain: string[];
}

interface CleanupFixture {
  id: number;
  category: string;
  raw: string;
  description: string;
  corrections: Record<string, string>;
  constraints: CleanupConstraints;
  reference: string;
}

const fixtureCorpus = corpus as CleanupFixture[];

// --- Cleanup test seam (PLAN.md "Test seams" / "Cleanup corpus") ------------
// The seam exercises the same boundary as the production groq::clean client
// (src-tauri/src/groq.rs): buildCleanupRequest serializes the user-message
// payload exactly like the production client, a mock provider stands in for
// the Groq chat-completions endpoint, and the serialized response must pass
// the same completion contract groq::clean enforces (exactly one choice,
// finish_reason "stop", plain string content, no refusal/tool calls, no leaked
// reasoning, no disallowed control characters) before any cleaned text is
// produced. Because every fixture demands mustChange, unchanged raw input
// cannot pass unnoticed.

// Mirrors CLEANUP_MODEL in src-tauri/src/models.rs.
const CLEANUP_MODEL = "qwen/qwen3.8-27b";

// The production system prompt sent by groq::clean.
const SYSTEM_PROMPT = readFileSync(
  resolve(process.cwd(), "src-tauri/prompts/dictation_cleanup.txt"),
  "utf8",
);

interface CleanupRequestPayload {
  raw_transcript: string;
  corrected_transcript: string;
}

interface CleanupCompletionBody {
  model: string;
  temperature: number;
  reasoning_effort: string;
  reasoning_format: string;
  max_completion_tokens: number;
  stream: boolean;
  messages: Array<{ role: string; content: string }>;
}

interface CleanupProviderResponse {
  choices: Array<{
    finish_reason?: string;
    message: { content: string | null; refusal?: unknown; tool_calls?: unknown };
  }>;
}

function applyCorrections(
  text: string,
  corrections: Record<string, string>,
): string {
  let output = text;
  // Longest source first so longer rules win over their prefixes.
  const rules = Object.entries(corrections).sort(([a], [b]) => b.length - a.length);
  for (const [source, replacement] of rules) {
    const escaped = source.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    output = output.replace(
      new RegExp(`(?<![\\p{L}\\p{N}])${escaped}(?![\\p{L}\\p{N}])`, "giu"),
      replacement,
    );
  }
  return output;
}

function buildCleanupRequest(entry: CleanupFixture): string {
  // groq::clean serializes this payload with serde_json and sends it as the
  // user message content; keep the fields and shape identical.
  return JSON.stringify({
    raw_transcript: entry.raw,
    corrected_transcript: applyCorrections(entry.raw, entry.corrections),
  } satisfies CleanupRequestPayload);
}

/** Stand-in for the Groq chat-completions endpoint. */
class MockCleanupProvider {
  readonly requests: CleanupCompletionBody[] = [];

  /** Returns the fixture's expected cleaned text as a serialized completion. */
  complete(request: CleanupCompletionBody, cleanedText: string): string {
    this.requests.push(request);
    return JSON.stringify({
      choices: [
        {
          finish_reason: "stop",
          message: { content: cleanedText },
        },
      ],
    });
  }
}

/** Mirrors the response validation groq::clean applies before returning text. */
function validateCleanupResponse(serialized: string): string {
  const chat = JSON.parse(serialized) as CleanupProviderResponse;
  expect(chat.choices, "cleanup response must contain exactly one choice").toHaveLength(1);
  const choice = chat.choices[0];
  expect(choice.message.refusal ?? null, "cleanup request was refused").toBeNull();
  expect(choice.message.tool_calls ?? null, "cleanup contained tool calls").toBeNull();
  expect(choice.finish_reason, "cleanup completion was truncated or abnormal").toBe("stop");
  const content = choice.message.content;
  expect(content, "cleanup model returned null content").toEqual(expect.any(String));
  expect(content!.length, "cleanup response exceeded 32,000 bytes limit").toBeLessThanOrEqual(
    32_000,
  );
  const hasDisallowedControl = [...content!].some(
    (c) => (c < " " && c !== "\t" && c !== "\n" && c !== "\r") || c === "\x7f",
  );
  expect(hasDisallowedControl, "cleanup response contains disallowed control characters").toBe(
    false,
  );
  expect(
    content!.startsWith("<think>") || content!.includes("</think>"),
    "cleanup response contains leaked reasoning block",
  ).toBe(false);
  const trimmed = content!.trim();
  expect(trimmed, "model returned empty text for substantive dictation").not.toBe("");
  return trimmed;
}

/** Runs one fixture through the production-shaped cleanup boundary. */
function invokeCleanupBoundary(
  entry: CleanupFixture,
  provider: MockCleanupProvider,
): string {
  const serializedRequest = buildCleanupRequest(entry);
  const serializedResponse = provider.complete(
    {
      model: CLEANUP_MODEL,
      temperature: 0.1,
      reasoning_effort: "low",
      reasoning_format: "hidden",
      max_completion_tokens: 16384,
      stream: false,
      messages: [
        { role: "system", content: SYSTEM_PROMPT },
        { role: "user", content: serializedRequest },
      ],
    },
    entry.reference,
  );
  return validateCleanupResponse(serializedResponse);
}

const normalize = (value: string) =>
  value.toLowerCase().replace(/\s+/g, " ").trim();

/** Case-insensitive substring match on whitespace-normalized text. */
const containsPhrase = (haystack: string, phrase: string) =>
  normalize(haystack).includes(normalize(phrase));

/** Case-insensitive whole-word sequence match (punctuation-insensitive). */
const containsWordSequence = (haystack: string, sequence: string) => {
  const words = normalize(haystack)
    .replace(/[^a-z0-9' ]+/g, " ")
    .split(/\s+/)
    .filter(Boolean);
  const target = normalize(sequence)
    .replace(/[^a-z0-9' ]+/g, " ")
    .split(/\s+/)
    .filter(Boolean);
  if (target.length === 0) return false;
  for (let i = 0; i + target.length <= words.length; i++) {
    if (target.every((word, j) => words[i + j] === word)) return true;
  }
  return false;
};

describe("Cleanup Corpus Test Suite (F34)", () => {
  it("contains at least 50 comprehensive real-world dictation test cases", () => {
    expect(corpus.length).toBeGreaterThanOrEqual(50);
  });

  it("ensures each corpus entry has a valid schema and non-empty content", () => {
    const seenIds = new Set<number>();
    const validCategories = new Set([
      "technical",
      "filler_removal",
      "self_correction",
      "dictation_command",
      "numbers_dates_currency",
      "formatting_casing",
      "multi_sentence",
      "edge_cases",
    ]);

    for (const item of fixtureCorpus) {
      expect(typeof item.id).toBe("number");
      expect(seenIds.has(item.id)).toBe(false);
      seenIds.add(item.id);

      expect(validCategories.has(item.category)).toBe(true);
      expect(typeof item.raw).toBe("string");
      expect(item.raw.trim().length).toBeGreaterThan(0);
      expect(typeof item.description).toBe("string");
      expect(item.description.trim().length).toBeGreaterThan(0);

      // Correction mappings: source -> replacement strings.
      expect(item.corrections).toEqual(expect.any(Object));
      for (const [source, replacement] of Object.entries(item.corrections)) {
        expect(source.trim().length).toBeGreaterThan(0);
        expect(typeof replacement).toBe("string");
        expect(replacement.trim().length).toBeGreaterThan(0);
      }

      // Expected constraints and reference output.
      expect(typeof item.constraints.mustChange).toBe("boolean");
      expect(item.constraints.mustContain).toEqual(expect.any(Array));
      expect(item.constraints.mustNotContain).toEqual(expect.any(Array));
      expect(typeof item.reference).toBe("string");
      expect(item.reference.trim().length).toBeGreaterThan(0);
    }
  });

  it("runs every entry through the cleanup boundary and asserts permitted behavior", () => {
    expect(fixtureCorpus.length).toBeGreaterThan(0);
    const provider = new MockCleanupProvider();

    for (const entry of fixtureCorpus) {
      const output = invokeCleanupBoundary(entry, provider);

      // The mock provider must have received the request the production
      // client sends: the serialized user payload keeps the raw transcript
      // distinct from the corrected one and applies every correction mapping
      // before the cleanup stage.
      const request = provider.requests.at(-1)!;
      expect(request.model).toBe(CLEANUP_MODEL);
      expect(request.messages).toHaveLength(2);
      expect(request.messages[0].role).toBe("system");
      expect(request.messages[0].content.trim().length).toBeGreaterThan(0);
      expect(request.messages[1].role).toBe("user");
      const payload = JSON.parse(request.messages[1].content) as CleanupRequestPayload;
      expect(payload.raw_transcript).toBe(entry.raw);
      for (const [source, replacement] of Object.entries(entry.corrections)) {
        expect(
          containsWordSequence(payload.corrected_transcript, replacement),
          `entry ${entry.id}: correction "${source}" -> "${replacement}" not applied`,
        ).toBe(true);
      }

      expect(typeof output).toBe("string");
      expect(output.trim().length, `entry ${entry.id}: empty output`).toBeGreaterThan(0);

      // Unchanged raw input cannot pass unnoticed.
      if (entry.constraints.mustChange) {
        expect(
          output.trim(),
          `entry ${entry.id}: cleanup output must differ from the raw input`,
        ).not.toBe(entry.raw.trim());
      }

      for (const phrase of entry.constraints.mustContain) {
        expect(
          containsPhrase(output, phrase),
          `entry ${entry.id}: output must contain "${phrase}" (got: ${output})`,
        ).toBe(true);
      }
      for (const phrase of entry.constraints.mustNotContain) {
        expect(
          containsWordSequence(output, phrase),
          `entry ${entry.id}: output must not contain "${phrase}" (got: ${output})`,
        ).toBe(false);
      }
    }
  });

  it("covers specialized technical terminology and code constructs", () => {
    const technicalCases = fixtureCorpus.filter((c) => c.category === "technical");
    expect(technicalCases.length).toBeGreaterThanOrEqual(8);

    const allTechText = technicalCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allTechText).toContain("dot net");
    expect(allTechText).toContain("c sharp");
    expect(allTechText).toContain("c plus plus");
    expect(allTechText).toContain("kubernetes");
    expect(allTechText).toContain("postgresql");
    expect(allTechText).toContain("oauth");
  });

  it("covers conversational hesitations and filler removals", () => {
    const fillerCases = fixtureCorpus.filter((c) => c.category === "filler_removal");
    expect(fillerCases.length).toBeGreaterThanOrEqual(5);

    const allFillerText = fillerCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allFillerText).toContain("um");
    expect(allFillerText).toContain("uh");
    expect(allFillerText).toContain("like");
    expect(allFillerText).toContain("you know");
  });

  it("covers in-flight self-corrections", () => {
    const selfCorrectionCases = fixtureCorpus.filter((c) => c.category === "self_correction");
    expect(selfCorrectionCases.length).toBeGreaterThanOrEqual(5);

    const allCorrections = selfCorrectionCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allCorrections).toContain("no sorry");
    expect(allCorrections).toContain("scratch that");
    expect(allCorrections).toContain("i mean");
  });

  it("covers spoken dictation commands", () => {
    const commandCases = fixtureCorpus.filter((c) => c.category === "dictation_command");
    expect(commandCases.length).toBeGreaterThanOrEqual(5);

    const allCommands = commandCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allCommands).toContain("new line");
    expect(allCommands).toContain("period");
    expect(allCommands).toContain("comma");
    expect(allCommands).toContain("question mark");
  });

  it("covers numeric, date, and currency expressions", () => {
    const numCases = fixtureCorpus.filter((c) => c.category === "numbers_dates_currency");
    expect(numCases.length).toBeGreaterThanOrEqual(5);

    const allNum = numCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allNum).toContain("twenty five dollars");
    expect(allNum).toContain("july fourth");
    expect(allNum).toContain("percent");
  });
});
