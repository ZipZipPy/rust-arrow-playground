import {
  createContext,
  type FocusEvent,
  type PointerEvent,
  type ReactNode,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';

/** Where to place the tooltip, in viewport coordinates. */
interface Anchor {
  x: number;
  top: number;
  bottom: number;
}

interface TooltipApi {
  show(content: ReactNode, anchor: Anchor): void;
  hide(): void;
}

const TooltipContext = createContext<TooltipApi | null>(null);
const MARGIN = 8;

/** One shared, viewport-clamped tooltip for charts and info buttons. */
export function TooltipProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<{ content: ReactNode; anchor: Anchor } | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!state || !el) return;
    const { width, height } = el.getBoundingClientRect();
    const left = Math.min(Math.max(MARGIN, state.anchor.x - width / 2), window.innerWidth - width - MARGIN);
    const above = state.anchor.top - height - MARGIN;
    const top = above >= MARGIN ? above : state.anchor.bottom + MARGIN;
    el.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
    el.style.visibility = 'visible';
  }, [state]);

  const api = useMemo<TooltipApi>(
    () => ({
      show: (content, anchor) => setState({ content, anchor }),
      hide: () => setState(null),
    }),
    [],
  );

  return (
    <TooltipContext.Provider value={api}>
      {children}
      {state && (
        <div ref={ref} className="tooltip" role="tooltip" style={{ visibility: 'hidden' }}>
          {state.content}
        </div>
      )}
    </TooltipContext.Provider>
  );
}

function useTooltipApi(): TooltipApi {
  const api = useContext(TooltipContext);
  if (!api) throw new Error('useTooltip must be used inside <TooltipProvider>');
  return api;
}

const anchorOf = (el: Element): Anchor => {
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2, top: r.top, bottom: r.bottom };
};

/**
 * Event handlers that show `content` while a mark is hovered (following the
 * pointer) or focused (anchored to the element).
 */
export function useTooltip() {
  const api = useTooltipApi();
  return useCallback(
    (content: ReactNode) => ({
      onPointerMove: (e: PointerEvent) => api.show(content, { x: e.clientX, top: e.clientY - 12, bottom: e.clientY + 20 }),
      onPointerLeave: () => api.hide(),
      onFocus: (e: FocusEvent) => api.show(content, anchorOf(e.currentTarget)),
      onBlur: () => api.hide(),
    }),
    [api],
  );
}

/** For info buttons: anchored to the element on hover, focus and tap. */
export function useAnchoredTooltip() {
  const api = useTooltipApi();
  return useCallback(
    (content: ReactNode) => {
      const show = (e: { currentTarget: Element }) => api.show(content, anchorOf(e.currentTarget));
      return {
        onPointerEnter: show,
        onPointerLeave: () => api.hide(),
        onFocus: show,
        onBlur: () => api.hide(),
        onClick: show,
      };
    },
    [api],
  );
}
