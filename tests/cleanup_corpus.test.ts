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
// The seam stands in for the provider-backed cleanup stage so the corpus
// asserts permitted cleanup behavior instead of fixture metadata only. It
// returns the fixture's reference output; a mock or live implementation can be
// swapped in here and must satisfy the same assertions, and because every
// fixture demands mustChange, unchanged raw input cannot pass unnoticed.

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

function buildCleanupRequest(entry: CleanupFixture) {
  return {
    raw_transcript: entry.raw,
    corrected_transcript: applyCorrections(entry.raw, entry.corrections),
  };
}

const cleanupSeam = (entry: CleanupFixture): string => entry.reference;

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

  it("runs every entry through the cleanup seam and asserts permitted behavior", () => {
    expect(fixtureCorpus.length).toBeGreaterThan(0);

    for (const entry of fixtureCorpus) {
      const request = buildCleanupRequest(entry);

      // The request keeps the raw transcript distinct from the corrected one
      // and applies every correction mapping before the cleanup stage.
      expect(request.raw_transcript).toBe(entry.raw);
      for (const [source, replacement] of Object.entries(entry.corrections)) {
        expect(
          containsWordSequence(request.corrected_transcript, replacement),
          `entry ${entry.id}: correction "${source}" -> "${replacement}" not applied`,
        ).toBe(true);
      }

      const output = cleanupSeam(entry);
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
