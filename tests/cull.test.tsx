import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import Cull from "../src/components/Cull";

const mocks = vi.hoisted(() => ({
  deleteScreenshots: vi.fn(), restoreScreenshots: vi.fn(), canRestoreTrash: vi.fn(),
}));
vi.mock("../src/api", () => ({
  api: mocks,
  canRestoreTrash: mocks.canRestoreTrash,
  thumbnailUrl: () => Promise.resolve(null),
}));
const items = [
  { id: 1, filename: "one.png", content_hash: null },
  { id: 2, filename: "two.png", content_hash: null },
];
const deleted = { trashed: 1, already_missing: 0, failed: [] };
afterEach(cleanup);
beforeEach(() => {
  vi.resetAllMocks();
  mocks.canRestoreTrash.mockReturnValue(true);
  mocks.deleteScreenshots.mockResolvedValue(deleted);
  mocks.restoreScreenshots.mockResolvedValue({ restored: 1, already_there: 0, failed: [] });
});

it("locks navigation and exit while trash is in flight, then reports the deleted id", async () => {
  let finish!: (value: typeof deleted) => void;
  mocks.deleteScreenshots.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const onDone = vi.fn();
  render(<Cull items={items} onDone={onDone} />);
  fireEvent.keyDown(window, { key: "x" });
  fireEvent.keyDown(window, { key: "ArrowRight" });
  fireEvent.keyDown(window, { key: "x" });
  fireEvent.keyDown(window, { key: "Escape" });
  fireEvent.click(screen.getByRole("button", { name: "Finish culling" }));
  expect(mocks.deleteScreenshots).toHaveBeenCalledTimes(1);
  expect(screen.getByText("one.png")).toBeTruthy();
  expect(onDone).not.toHaveBeenCalled();
  await act(async () => finish(deleted));
  expect(screen.getByText("two.png")).toBeTruthy();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onDone).toHaveBeenCalledWith([1]);
});

it("keeps a failed deletion visible and out of the undo stack", async () => {
  mocks.deleteScreenshots.mockResolvedValue({ ...deleted, trashed: 0, failed: [{ id: 1, message: "Permission denied" }] });
  render(<Cull items={items} onDone={vi.fn()} />);
  fireEvent.keyDown(window, { key: "x" });
  await screen.findByText("Permission denied");
  expect(screen.getByText("one.png")).toBeTruthy();
  fireEvent.keyDown(window, { key: "u" });
  expect(mocks.restoreScreenshots).not.toHaveBeenCalled();
});

it("undo restores the final trashed screenshot and returns to its preview", async () => {
  const onDone = vi.fn();
  render(<Cull items={items.slice(0, 1)} onDone={onDone} />);
  fireEvent.keyDown(window, { key: "x" });
  await screen.findByText("Reviewed them all.");
  fireEvent.keyDown(window, { key: "u" });
  await screen.findByText("one.png");
  expect(mocks.restoreScreenshots).toHaveBeenCalledWith([1]);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onDone).toHaveBeenCalledWith([]);
});

it("stays on the last screenshot when keeping it", async () => {
  render(<Cull items={items} onDone={vi.fn()} />);
  fireEvent.keyDown(window, { key: "ArrowRight" });
  fireEvent.keyDown(window, { key: " " });
  await waitFor(() => expect(screen.getByText("two.png")).toBeTruthy());
  expect(screen.queryByText("Reviewed them all.")).toBeNull();
});

it("shows manual restore guidance on platforms without in-app restore", async () => {
  mocks.canRestoreTrash.mockReturnValue(false);
  render(<Cull items={items} onDone={vi.fn()} />);
  expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
  expect(screen.getByText(/Restore from Finder/)).toBeTruthy();
  fireEvent.keyDown(window, { key: "x" });
  await screen.findByText("two.png");
  fireEvent.keyDown(window, { key: "u" });
  expect(mocks.restoreScreenshots).not.toHaveBeenCalled();
});
