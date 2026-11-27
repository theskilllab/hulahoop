import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import Select, { type SelectOption } from "./Select";

// Contract for the shared searchable dropdown: `<Select label value options onChange />`.
// The trigger is a button with aria-haspopup="listbox"; the popover is `.sel-pop`
// with role="listbox" and role="option" rows. No native <select> menu is used.

const few: SelectOption[] = [
  { value: "plan", label: "Plan" },
  { value: "spec", label: "Spec" },
  { value: "report", label: "Report", disabled: true },
];
const many: SelectOption[] = [
  { value: "a", label: "Alpha", group: "Tasks" },
  { value: "b", label: "Bravo", group: "Tasks" },
  { value: "c", label: "Charlie", group: "Milestones" },
  { value: "d", label: "Delta", group: "Milestones" },
  { value: "e", label: "Echo", group: "Milestones" },
];

function setup(options: SelectOption[], value = options[0].value) {
  const onChange = vi.fn();
  const view = render(<Select label="Type" value={value} options={options} onChange={onChange} />);
  const trigger = screen.getByRole("button", { name: /Type/ });
  return { onChange, trigger, view };
}

describe("Select", () => {
  it("renders no native select and starts closed", () => {
    const { view } = setup(few);
    expect(view.container.querySelector("select")).toBeNull();
    expect(document.querySelector(".sel-pop")).toBeNull();
  });

  it("opens on click, Space, Enter and the arrow keys", async () => {
    const { trigger } = setup(few);
    await userEvent.click(trigger);
    expect(document.querySelector(".sel-pop")).toBeTruthy();
    await userEvent.keyboard("{Escape}");
    for (const key of [" ", "{Enter}", "{ArrowDown}", "{ArrowUp}"]) {
      trigger.focus();
      await userEvent.keyboard(key);
      expect(document.querySelector(".sel-pop"), key).toBeTruthy();
      await userEvent.keyboard("{Escape}");
      expect(document.querySelector(".sel-pop"), key).toBeNull();
    }
  });

  it("marks the current choice and shows disabled options at reduced opacity", async () => {
    const { trigger } = setup(few, "spec");
    await userEvent.click(trigger);
    const current = screen.getByRole("option", { name: /Spec/ });
    expect(current.getAttribute("aria-selected")).toBe("true");
    expect(current.querySelector("svg")).toBeTruthy();
    expect(screen.getByRole("option", { name: /Report/ }).getAttribute("aria-disabled")).toBe("true");
  });

  it("moves with the arrows, chooses with Enter and returns focus", async () => {
    const { trigger, onChange } = setup(few);
    await userEvent.click(trigger);
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalledWith("spec");
    expect(document.querySelector(".sel-pop")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("does not choose a disabled option", async () => {
    const { trigger, onChange } = setup(few);
    await userEvent.click(trigger);
    await userEvent.click(screen.getByRole("option", { name: /Report/ }));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("Escape closes and returns focus; Tab closes", async () => {
    const { trigger } = setup(few);
    await userEvent.click(trigger);
    await userEvent.keyboard("{Escape}");
    expect(document.querySelector(".sel-pop")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    await userEvent.click(trigger);
    await userEvent.keyboard("{Tab}");
    expect(document.querySelector(".sel-pop")).toBeNull();
  });

  it("closes on an outside click", async () => {
    const { trigger } = setup(few);
    await userEvent.click(trigger);
    fireEvent.mouseDown(document.body);
    expect(document.querySelector(".sel-pop")).toBeNull();
  });

  it("without search, typing a letter jumps to the next option starting with it", async () => {
    const { trigger, onChange } = setup(few);
    await userEvent.click(trigger);
    expect(screen.queryByPlaceholderText("Search…")).toBeNull();
    await userEvent.keyboard("s{Enter}");
    expect(onChange).toHaveBeenCalledWith("spec");
  });

  it("with more than 4 options shows group headings and a search that filters", async () => {
    const { trigger, onChange } = setup(many);
    await userEvent.click(trigger);
    const pop = document.querySelector(".sel-pop") as HTMLElement;
    expect(within(pop).getByText("Tasks")).toBeTruthy();
    expect(within(pop).getByText("Milestones")).toBeTruthy();
    const search = screen.getByPlaceholderText("Search…");
    await userEvent.type(search, "cha");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await userEvent.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith("c");
  });

  it("says No matches when the search finds nothing", async () => {
    const { trigger } = setup(many);
    await userEvent.click(trigger);
    await userEvent.type(screen.getByPlaceholderText("Search…"), "zzz");
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("No matches")).toBeTruthy();
  });

  it("does not show a search row for 4 options or fewer", async () => {
    const { trigger } = setup(many.slice(0, 4));
    await userEvent.click(trigger);
    expect(screen.queryByPlaceholderText("Search…")).toBeNull();
  });
});

describe("Select extensions", () => {
  it("renders an option's detail text", async () => {
    const options: SelectOption[] = [
      { value: "a", label: "Alpha", detail: "$3 / $15" },
      { value: "b", label: "Bravo", disabled: true, detail: "not connected" },
    ];
    setup(options);
    await userEvent.click(screen.getByRole("button", { name: /Type/ }));
    expect(document.querySelector(".sel-pop .sp-detail")?.textContent).toBe("$3 / $15");
    expect(screen.getByText("not connected")).toBeTruthy();
  });

  it("a disabled Select does not open and accepts an id", async () => {
    render(<Select label="Type" id="t1" title="No moves" disabled value="a" options={few} onChange={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: /Type/ }) as HTMLButtonElement;
    expect(trigger.id).toBe("t1");
    expect(trigger.getAttribute("title")).toBe("No moves");
    expect(trigger.disabled).toBe(true);
    await userEvent.click(trigger);
    expect(document.querySelector(".sel-pop")).toBeNull();
  });

  it("a printable key in a non-searchable popover is consumed so global shortcuts do not fire", async () => {
    const { trigger } = setup(few);
    await userEvent.click(trigger);
    const pop = document.querySelector(".sel-pop") as HTMLElement;
    expect(fireEvent.keyDown(pop, { key: "c" })).toBe(false);
  });

  it("search matches the group name", async () => {
    const { trigger } = setup(many);
    await userEvent.click(trigger);
    await userEvent.type(screen.getByPlaceholderText("Search…"), "milestones");
    expect(screen.getAllByRole("option")).toHaveLength(3);
  });
});
