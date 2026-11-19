// Tooltips for the call window. It floats over other apps without taking
// focus, and macOS only shows native tooltips (`title`) in the active app,
// so these are drawn here: anything with `data-tip` gets one after a short
// hover (straight away while moving between buttons), or on keyboard focus.
import { useEffect, useLayoutEffect, useRef, useState } from "react";

const DELAY_MS = 350;
/** Moving to the next button within this keeps tooltips coming instantly. */
const WARM_MS = 600;
const MARGIN = 6;

export interface Tip {
  el: HTMLElement;
  text: string;
}

/** The tooltip to show right now, if any. */
export function useTooltip(): Tip | null {
  const [tip, setTip] = useState<Tip | null>(null);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let shown: HTMLElement | null = null;
    let lastHidden = 0;
    /** Focus shows a tooltip only when it came from the keyboard. */
    let keyboard = false;
    const text = (el: HTMLElement) => el.getAttribute("data-tip") ?? "";
    const show = (el: HTMLElement) => {
      shown = el;
      setTip({ el, text: text(el) });
    };
    const hide = () => {
      clearTimeout(timer);
      if (shown) lastHidden = performance.now();
      shown = null;
      setTip(null);
    };
    const over = (e: Event) => {
      const el = (e.target as Element | null)?.closest?.<HTMLElement>("[data-tip]") ?? null;
      if (el === shown) return;
      clearTimeout(timer);
      if (!el) return hide();
      const warm = shown !== null || performance.now() - lastHidden < WARM_MS;
      if (shown) {
        lastHidden = performance.now();
        shown = null;
        setTip(null);
      }
      if (warm) show(el);
      else timer = setTimeout(() => el.isConnected && show(el), DELAY_MS);
    };
    const out = (e: PointerEvent) => {
      const to = e.relatedTarget as Node | null;
      if (!to || !document.documentElement.contains(to)) hide(); // left the window
    };
    const focus = (e: FocusEvent) => {
      const el = (e.target as Element | null)?.closest?.<HTMLElement>("[data-tip]");
      if (el && keyboard) show(el);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Tab") keyboard = true;
    };
    const press = () => {
      keyboard = false;
      hide();
    };
    document.addEventListener("pointerover", over);
    document.addEventListener("pointerout", out);
    document.addEventListener("pointerdown", press, true);
    document.addEventListener("keydown", key, true);
    document.addEventListener("focusin", focus);
    document.addEventListener("focusout", hide);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("pointerover", over);
      document.removeEventListener("pointerout", out);
      document.removeEventListener("pointerdown", press, true);
      document.removeEventListener("keydown", key, true);
      document.removeEventListener("focusin", focus);
      document.removeEventListener("focusout", hide);
    };
  }, []);
  // Its element went (the strip expanded, a menu closed): no tooltip.
  useEffect(() => {
    if (tip && !tip.el.isConnected) setTip(null);
  });
  if (tip && !tip.el.isConnected) return null;
  return tip ? { el: tip.el, text: tip.el.getAttribute("data-tip") ?? tip.text } : null;
}

/** The tooltip, above its element (below if there's no room), kept inside
 * the window. */
export function TooltipLayer({ tip }: { tip: Tip | null }) {
  const box = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useLayoutEffect(() => {
    const el = box.current;
    if (!tip || !el) return setPos(null);
    const r = tip.el.getBoundingClientRect();
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const vw = document.documentElement.clientWidth || window.innerWidth;
    const left = Math.min(Math.max(MARGIN, r.left + r.width / 2 - w / 2), Math.max(MARGIN, vw - w - MARGIN));
    const above = r.top - h - MARGIN;
    const top = above >= MARGIN ? above : r.bottom + MARGIN;
    setPos({ left, top });
  }, [tip?.el, tip?.text]);
  if (!tip?.text) return null;
  return (
    <div
      ref={box}
      className="tip"
      role="tooltip"
      style={pos ? { left: pos.left, top: pos.top } : { left: 0, top: 0, visibility: "hidden" }}
    >
      {tip.text}
    </div>
  );
}
