/* Samlu's dropdowns.
 *
 * Every <select> on a page that loads this file is drawn as a Samlu control
 * instead of the system one: a pill trigger with a soft menu, or, with
 * data-variant="segmented", a row of segments for a short set of choices.
 *
 * The native <select> stays in the DOM, hidden, and remains the source of
 * truth. Scripts keep reading and writing `select.value` and listening for
 * `change` exactly as before; this file mirrors those writes and dispatches a
 * real `change` when the person picks something. Options added, removed,
 * disabled or relabelled later are picked up by a MutationObserver. */
(function () {
  const VALUE = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value");
  const INDEX = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "selectedIndex");
  const CHEVRON =
    '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M3.2 4.6 6 7.4l2.8-2.8"/></svg>';
  const CHECK =
    '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 6.2l2.3 2.3 4.7-5"/></svg>';

  let uid = 0;
  let openControl = null;

  function labelFor(select) {
    if (select.getAttribute("aria-label")) return select.getAttribute("aria-label");
    if (select.id) {
      const explicit = document.querySelector(`label[for="${CSS.escape(select.id)}"]`);
      if (explicit) return explicit.textContent.trim();
    }
    const wrapping = select.closest("label");
    const name = wrapping?.querySelector("strong, .field > span, span");
    return (name || wrapping)?.textContent.trim().split("\n")[0] || "";
  }

  /** The select's options as plain data, with optgroup headings in place. */
  function readItems(select) {
    const items = [];
    for (const child of select.children) {
      if (child.tagName === "OPTGROUP") {
        const options = [...child.children].filter((option) => !option.hidden);
        if (!options.length) continue;
        items.push({ heading: child.label });
        options.forEach((option) => items.push(optionItem(option, child.disabled)));
      } else if (child.tagName === "OPTION" && !child.hidden) {
        items.push(optionItem(child, false));
      }
    }
    return items;
  }

  function optionItem(option, groupDisabled) {
    return {
      value: option.value,
      label: option.textContent.trim(),
      disabled: option.disabled || groupDisabled,
      option,
    };
  }

  function commit(select, value) {
    if (VALUE.get.call(select) === value) return;
    VALUE.set.call(select, value);
    select.dispatchEvent(new Event("input", { bubbles: true }));
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }

  /* ------------------------------------------------------------ dropdown */

  function enhanceDropdown(select) {
    const id = `samlu-select-${++uid}`;
    const trigger = document.createElement("button");
    trigger.type = "button";
    trigger.className = "samlu-select";
    trigger.setAttribute("aria-haspopup", "listbox");
    trigger.setAttribute("aria-expanded", "false");
    trigger.setAttribute("aria-controls", `${id}-menu`);
    trigger.innerHTML = `<span class="samlu-select-value"></span>${CHEVRON}`;
    const valueLabel = trigger.firstChild;

    const menu = document.createElement("div");
    menu.className = "samlu-select-menu";
    menu.id = `${id}-menu`;
    menu.setAttribute("role", "listbox");
    menu.tabIndex = -1;
    menu.hidden = true;
    document.body.appendChild(menu);

    let items = [];
    let active = -1;
    let typed = "";
    let typedAt = 0;

    function choices() {
      return items.filter((item) => !item.heading);
    }

    function sync() {
      const current = VALUE.get.call(select);
      const selected = choices().find((item) => item.value === current);
      valueLabel.textContent = selected ? selected.label : select.dataset.placeholder || "Choose…";
      trigger.disabled = select.disabled;
      trigger.setAttribute("aria-label", `${labelFor(select)}: ${valueLabel.textContent}`);
      menu.querySelectorAll("[role=option]").forEach((row) => {
        const on = row.dataset.value === current;
        row.setAttribute("aria-selected", String(on));
      });
    }

    function build() {
      items = readItems(select);
      menu.replaceChildren();
      menu.setAttribute("aria-label", labelFor(select));
      let optionIndex = 0;
      items.forEach((item) => {
        if (item.heading) {
          const heading = document.createElement("div");
          heading.className = "samlu-select-heading";
          heading.setAttribute("role", "presentation");
          heading.textContent = item.heading;
          menu.appendChild(heading);
          return;
        }
        const row = document.createElement("div");
        row.className = "samlu-select-option";
        row.id = `${id}-option-${optionIndex}`;
        row.dataset.index = String(optionIndex++);
        row.dataset.value = item.value;
        row.setAttribute("role", "option");
        if (item.disabled) row.setAttribute("aria-disabled", "true");
        row.innerHTML = `<span class="samlu-select-check">${CHECK}</span><span></span>`;
        row.lastChild.textContent = item.label;
        row.addEventListener("pointermove", () => setActive(Number(row.dataset.index), false));
        row.addEventListener("click", (event) => {
          event.preventDefault();
          if (item.disabled) return;
          commit(select, item.value);
          close(true);
        });
        menu.appendChild(row);
      });
      sync();
    }

    function setActive(index, scroll = true) {
      const list = choices();
      if (!list.length) return;
      active = Math.max(0, Math.min(list.length - 1, index));
      menu.querySelectorAll("[role=option]").forEach((row) => {
        row.dataset.active = String(Number(row.dataset.index) === active);
      });
      const row = menu.querySelector(`[data-index="${active}"]`);
      if (row) {
        menu.setAttribute("aria-activedescendant", row.id);
        // Scroll only the menu's own list: scrollIntoView would also nudge the
        // page, and a page scroll closes the menu.
        if (scroll) {
          if (row.offsetTop < menu.scrollTop) menu.scrollTop = row.offsetTop - 5;
          else if (row.offsetTop + row.offsetHeight > menu.scrollTop + menu.clientHeight) {
            menu.scrollTop = row.offsetTop + row.offsetHeight - menu.clientHeight + 5;
          }
        }
      }
    }

    function step(from, direction) {
      const list = choices();
      let index = from;
      for (let tries = 0; tries < list.length; tries += 1) {
        index += direction;
        if (index < 0 || index >= list.length) return from;
        if (!list[index].disabled) return index;
      }
      return from;
    }

    function place() {
      const rect = trigger.getBoundingClientRect();
      const gap = 6;
      menu.style.minWidth = `${Math.round(rect.width)}px`;
      menu.style.left = `${Math.round(Math.min(rect.left, window.innerWidth - menu.offsetWidth - 8))}px`;
      const below = window.innerHeight - rect.bottom - gap - 8;
      const above = rect.top - gap - 8;
      const height = Math.min(menu.scrollHeight, 300);
      const flip = below < height && above > below;
      menu.style.maxHeight = `${Math.max(120, Math.min(300, flip ? above : below))}px`;
      menu.style.top = flip
        ? `${Math.round(rect.top - gap - Math.min(height, above))}px`
        : `${Math.round(rect.bottom + gap)}px`;
      menu.dataset.side = flip ? "top" : "bottom";
    }

    function open() {
      if (trigger.disabled || !menu.hidden) return;
      openControl?.close(false);
      openControl = control;
      build();
      menu.hidden = false;
      trigger.setAttribute("aria-expanded", "true");
      trigger.dataset.open = "true";
      place();
      const current = choices().findIndex((item) => item.value === VALUE.get.call(select));
      setActive(current >= 0 ? current : step(-1, 1));
      menu.focus({ preventScroll: true });
    }

    function close(refocus) {
      if (menu.hidden) return;
      menu.dataset.leaving = "true";
      trigger.setAttribute("aria-expanded", "false");
      delete trigger.dataset.open;
      if (openControl === control) openControl = null;
      setTimeout(() => {
        menu.hidden = true;
        delete menu.dataset.leaving;
      }, 120);
      if (refocus) trigger.focus({ preventScroll: true });
    }

    function typeahead(key) {
      const now = Date.now();
      typed = now - typedAt > 700 ? key : typed + key;
      typedAt = now;
      const list = choices();
      const match = list.findIndex(
        (item, index) => !item.disabled && item.label.toLowerCase().startsWith(typed.toLowerCase()) && (typed.length > 1 || index > active),
      );
      const fallback = list.findIndex((item) => !item.disabled && item.label.toLowerCase().startsWith(typed.toLowerCase()));
      const index = match >= 0 ? match : fallback;
      if (index >= 0) setActive(index);
    }

    trigger.addEventListener("click", () => (menu.hidden ? open() : close(true)));
    trigger.addEventListener("keydown", (event) => {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) {
        event.preventDefault();
        open();
      }
    });

    menu.addEventListener("keydown", (event) => {
      const list = choices();
      if (event.key === "ArrowDown") setActive(step(active, 1));
      else if (event.key === "ArrowUp") setActive(step(active, -1));
      else if (event.key === "Home") setActive(step(-1, 1));
      else if (event.key === "End") setActive(step(list.length, -1));
      else if (event.key === "Enter" || event.key === " ") {
        const item = list[active];
        if (item && !item.disabled) commit(select, item.value);
        close(true);
      } else if (event.key === "Escape") close(true);
      else if (event.key === "Tab") {
        close(false);
        return;
      } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey) typeahead(event.key);
      else return;
      event.preventDefault();
      event.stopPropagation();
    });

    menu.addEventListener("focusout", (event) => {
      if (!menu.contains(event.relatedTarget) && event.relatedTarget !== trigger) close(false);
    });

    const control = { close, sync, build, place, menu, trigger };
    return { element: trigger, control };
  }

  /* ----------------------------------------------------------- segmented */

  function enhanceSegmented(select) {
    const group = document.createElement("div");
    group.className = "samlu-segmented";
    group.setAttribute("role", "radiogroup");

    function sync() {
      const current = VALUE.get.call(select);
      group.setAttribute("aria-label", labelFor(select));
      group.querySelectorAll("button").forEach((button) => {
        const on = button.dataset.value === current;
        button.setAttribute("aria-checked", String(on));
        button.tabIndex = on ? 0 : -1;
        button.disabled = select.disabled || button.dataset.disabled === "true";
      });
      if (!group.querySelector('[aria-checked="true"]')) {
        const first = group.querySelector("button:not(:disabled)");
        if (first) first.tabIndex = 0;
      }
    }

    function build() {
      group.replaceChildren();
      readItems(select)
        .filter((item) => !item.heading)
        .forEach((item) => {
          const button = document.createElement("button");
          button.type = "button";
          button.setAttribute("role", "radio");
          button.dataset.value = item.value;
          button.dataset.disabled = String(item.disabled);
          // Long option labels ("Samlu Island — under the notch") keep only
          // their name in a segment; the full label stays as the tooltip.
          const short = item.option.dataset.short || item.label.split(" — ")[0];
          button.textContent = short;
          if (short !== item.label) button.title = item.label;
          button.addEventListener("click", () => {
            commit(select, item.value);
            sync();
          });
          group.appendChild(button);
        });
      sync();
    }

    group.addEventListener("keydown", (event) => {
      const buttons = [...group.querySelectorAll("button:not(:disabled)")];
      const index = buttons.indexOf(document.activeElement);
      if (index < 0) return;
      let next = -1;
      if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (index + 1) % buttons.length;
      if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (index - 1 + buttons.length) % buttons.length;
      if (next < 0) return;
      event.preventDefault();
      buttons[next].focus();
      buttons[next].click();
    });

    return { element: group, control: { sync, build, close() {}, place() {} } };
  }

  /* -------------------------------------------------------------- wiring */

  function enhance(select) {
    if (select.dataset.samluEnhanced) return;
    select.dataset.samluEnhanced = "true";
    const segmented = select.dataset.variant === "segmented";
    const { element, control } = segmented ? enhanceSegmented(select) : enhanceDropdown(select);

    select.classList.add("samlu-native");
    select.tabIndex = -1;
    select.setAttribute("aria-hidden", "true");
    select.insertAdjacentElement("afterend", element);
    control.build();

    // Mirror programmatic writes. Scripts set `.value` without an event, so
    // the instance property forwards to the native one and then repaints.
    Object.defineProperty(select, "value", {
      configurable: true,
      get() {
        return VALUE.get.call(this);
      },
      set(value) {
        VALUE.set.call(this, value);
        control.sync();
      },
    });
    Object.defineProperty(select, "selectedIndex", {
      configurable: true,
      get() {
        return INDEX.get.call(this);
      },
      set(index) {
        INDEX.set.call(this, index);
        control.sync();
      },
    });
    select.addEventListener("change", () => control.sync());

    new MutationObserver(() => control.build()).observe(select, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
      attributeFilter: ["disabled", "hidden", "label"],
    });
  }

  function enhanceAll(root = document) {
    root.querySelectorAll("select").forEach(enhance);
  }

  document.addEventListener("pointerdown", (event) => {
    if (!openControl) return;
    if (openControl.menu.contains(event.target) || openControl.trigger.contains(event.target)) return;
    openControl.close(false);
  });
  window.addEventListener("resize", () => openControl?.place());
  // The page scrolling moves the trigger out from under an open menu; the
  // menu scrolling its own list does not.
  document.addEventListener(
    "scroll",
    (event) => {
      if (openControl && !openControl.menu.contains(event.target)) openControl.close(false);
    },
    true,
  );

  window.SamluSelect = { enhance, enhanceAll };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", () => enhanceAll());
  } else {
    enhanceAll();
  }
})();
