import { describe, it, expect } from "vitest";
import corpus from "./fixtures/cleanup_corpus.json";

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

    for (const item of corpus) {
      expect(typeof item.id).toBe("number");
      expect(seenIds.has(item.id)).toBe(false);
      seenIds.add(item.id);

      expect(validCategories.has(item.category)).toBe(true);
      expect(typeof item.raw).toBe("string");
      expect(item.raw.trim().length).toBeGreaterThan(0);
      expect(typeof item.description).toBe("string");
      expect(item.description.trim().length).toBeGreaterThan(0);
    }
  });

  it("covers specialized technical terminology and code constructs", () => {
    const technicalCases = corpus.filter((c) => c.category === "technical");
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
    const fillerCases = corpus.filter((c) => c.category === "filler_removal");
    expect(fillerCases.length).toBeGreaterThanOrEqual(5);

    const allFillerText = fillerCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allFillerText).toContain("um");
    expect(allFillerText).toContain("uh");
    expect(allFillerText).toContain("like");
    expect(allFillerText).toContain("you know");
  });

  it("covers in-flight self-corrections", () => {
    const selfCorrectionCases = corpus.filter((c) => c.category === "self_correction");
    expect(selfCorrectionCases.length).toBeGreaterThanOrEqual(5);

    const allCorrections = selfCorrectionCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allCorrections).toContain("no sorry");
    expect(allCorrections).toContain("scratch that");
    expect(allCorrections).toContain("i mean");
  });

  it("covers spoken dictation commands", () => {
    const commandCases = corpus.filter((c) => c.category === "dictation_command");
    expect(commandCases.length).toBeGreaterThanOrEqual(5);

    const allCommands = commandCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allCommands).toContain("new line");
    expect(allCommands).toContain("period");
    expect(allCommands).toContain("comma");
    expect(allCommands).toContain("question mark");
  });

  it("covers numeric, date, and currency expressions", () => {
    const numCases = corpus.filter((c) => c.category === "numbers_dates_currency");
    expect(numCases.length).toBeGreaterThanOrEqual(5);

    const allNum = numCases.map((c) => c.raw.toLowerCase()).join(" ");
    expect(allNum).toContain("twenty five dollars");
    expect(allNum).toContain("july fourth");
    expect(allNum).toContain("percent");
  });
});
