document.querySelectorAll(".dusk-stack").forEach((stack) => {
  const parts = stack.querySelectorAll("[data-layer]");
  const diagram = stack.querySelector(".dusk-stack__diagram");
  const plates = [...diagram.querySelectorAll(".dusk-plate")];
  const fleet = diagram.querySelector(".dusk-stack__fleet");
  const visible = new Set();
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          visible.add(entry.target);
        } else {
          visible.delete(entry.target);
        }
      }
      const current = visible.values().next().value;
      const layer = current ? current.dataset.layer : "";
      stack.classList.toggle("is-focused", Boolean(current));
      for (const part of parts) {
        part.classList.toggle("is-active", part.dataset.layer === layer);
      }
      for (const plate of plates) {
        if (plate.dataset.layer !== layer) {
          diagram.insertBefore(plate, fleet);
        }
      }
      for (const plate of plates) {
        if (plate.dataset.layer === layer) {
          diagram.insertBefore(plate, fleet);
        }
      }
    },
    { rootMargin: "-49% 0px -50% 0px" },
  );
  for (const layer of stack.querySelectorAll(".dusk-layer")) {
    observer.observe(layer);
  }
});
