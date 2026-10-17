import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import StatusBar from "../src/components/StatusBar";

const mocks = vi.hoisted(() => ({ getStats: vi.fn(), startOcr: vi.fn(), progress: vi.fn(), complete: vi.fn() }));
vi.mock("../src/api", () => ({
  api: { getStats: mocks.getStats, startOcr: mocks.startOcr },
  onOcrProgress: mocks.progress, onOcrComplete: mocks.complete,
}));
afterEach(cleanup);
beforeEach(() => {
  vi.resetAllMocks();
  mocks.getStats.mockResolvedValue({ problem_count: 0, ocr_pending: 2, ocr_failed: 0 });
  mocks.progress.mockResolvedValue(vi.fn());
  mocks.complete.mockResolvedValue(vi.fn());
});
it("shows the missing OCR engine error even while the queue has pending work", async () => {
  mocks.startOcr.mockRejectedValue("OCR unavailable: install tesseract");
  render(<StatusBar appState={null} onRefresh={async () => null} />);
  fireEvent.click(await screen.findByRole("button", { name: "Start" }));
  expect(await screen.findByText("OCR unavailable: install tesseract")).toBeTruthy();
  expect(screen.getByText("2 awaiting text extraction")).toBeTruthy();
});
it("unsubscribes listeners that finish registration after unmount", async () => {
  let register!: (value: () => void) => void;
  const unlisten = vi.fn();
  mocks.progress.mockReturnValue(new Promise((resolve) => { register = resolve; }));
  const view = render(<StatusBar appState={null} onRefresh={async () => null} />);
  view.unmount();
  await act(async () => register(unlisten));
  expect(unlisten).toHaveBeenCalledTimes(1);
});
