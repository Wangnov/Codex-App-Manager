import { useId, useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { Sheet } from "./Sheet";

function SheetHarness({ dismissable = true }: { dismissable?: boolean }) {
  const [open, setOpen] = useState(false);
  const titleId = useId();
  const bodyId = useId();
  return (
    <>
      <button onClick={() => setOpen(true)}>Open</button>
      <Sheet
        open={open}
        onDismiss={() => setOpen(false)}
        dismissable={dismissable}
        labelledBy={titleId}
        describedBy={bodyId}
        initialFocus="dismiss"
      >
        <h3 id={titleId}>Danger zone</h3>
        <p id={bodyId}>Confirm the action</p>
        <div className="row2">
          <button className="btn ghost" onClick={() => setOpen(false)}>
            Cancel
          </button>
          <button className="btn danger">Delete</button>
        </div>
      </Sheet>
    </>
  );
}

describe("Sheet", () => {
  it("sets dialog semantics, traps focus, handles Escape, and restores focus", async () => {
    const user = userEvent.setup();
    render(<SheetHarness />);

    const opener = screen.getByRole("button", { name: "Open" });
    await user.click(opener);

    const dialog = screen.getByRole("dialog", { name: "Danger zone" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveAttribute("aria-describedby");
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus());

    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(screen.getByRole("button", { name: "Delete" })).toHaveFocus();
    await user.keyboard("{Tab}");
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(opener).toHaveFocus();
  });

  it("ignores Escape when dismissable is false", async () => {
    const user = userEvent.setup();
    const onError = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<SheetHarness dismissable={false} />);

    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.keyboard("{Escape}");

    expect(screen.getByRole("dialog", { name: "Danger zone" })).toBeInTheDocument();
    onError.mockRestore();
  });
});
