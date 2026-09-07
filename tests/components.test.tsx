import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { HistoryDetail } from "../src/components/HistoryDetail";
import { PrivacyNoticeModal } from "../src/components/PrivacyNoticeModal";
import type { HistoryEntry } from "../src/types";

describe("Frontend Modal Components", () => {
  it("renders HistoryDetail with raw and polished text and metadata", () => {
    const entry: HistoryEntry = {
      id: 101,
      uuid: "123e4567-e89b-12d3-a456-426614174000",
      text: "Hello team, let's ship this.",
      raw_text: "hello team lets ship this",
      word_count: 5,
      duration_ms: 2500,
      input_tokens: 15,
      output_tokens: 8,
      delivery_mode: "paste",
      created_at: 1788728580,
    };

    const ref = createRef<HTMLElement>();
    render(<HistoryDetail entry={entry} onClose={() => {}} returnFocusRef={ref} />);

    expect(screen.getByText("Dictation Details")).toBeInTheDocument();
    expect(screen.getByText("Polished Final Text")).toBeInTheDocument();
    expect(screen.getByText("Hello team, let's ship this.")).toBeInTheDocument();
    expect(screen.getByText("Raw Whisper Transcript")).toBeInTheDocument();
    expect(screen.getByText("hello team lets ship this")).toBeInTheDocument();
    expect(screen.getByText("Direct Paste")).toBeInTheDocument();
  });

  it("renders PrivacyNoticeModal and triggers onAccept", async () => {
    const user = userEvent.setup();
    const onAccept = vi.fn();
    const ref = createRef<HTMLElement>();

    render(<PrivacyNoticeModal open={true} onAccept={onAccept} returnFocusRef={ref} />);

    expect(screen.getByText("Privacy & Data Processing Notice")).toBeInTheDocument();
    expect(screen.getByText("Exclusive Remote Provider (Groq)")).toBeInTheDocument();

    const acceptButton = screen.getByRole("button", { name: /I Understand & Accept/i });
    await user.click(acceptButton);

    expect(onAccept).toHaveBeenCalledTimes(1);
  });
});
