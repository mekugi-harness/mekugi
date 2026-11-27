// Follow each scroll region until its reader scrolls away from the bottom.
export function followBottom(scroller: HTMLElement) {
  let following = true;
  let end = 0;
  const scroll = () => {
    // Layout can clamp the scroll position before ResizeObserver runs.
    if (end === scroller.scrollHeight - scroller.clientHeight)
      following = end - scroller.scrollTop < 40;
  };
  const observer = new ResizeObserver(() => {
    end = scroller.scrollHeight - scroller.clientHeight;
    if (following) scroller.scrollTop = scroller.scrollHeight;
  });
  observer.observe(scroller);
  observer.observe(scroller.firstElementChild!);
  scroller.addEventListener("scroll", scroll);
  return {
    destroy() {
      observer.disconnect();
      scroller.removeEventListener("scroll", scroll);
    },
  };
}
