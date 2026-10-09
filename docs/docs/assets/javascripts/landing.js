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

const visibility = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    entry.target.classList.toggle("is-offscreen", !entry.isIntersecting);
    for (const network of entry.target.querySelectorAll(
      ".dusk-fleet__network",
    )) {
      if (entry.isIntersecting) {
        network.unpauseAnimations();
      } else {
        network.pauseAnimations();
      }
    }
  }
});
for (const section of document.querySelectorAll(
  ".dusk-hero, .dusk-fleet, .dusk-stack",
)) {
  visibility.observe(section);
}

for (const sun of document.querySelectorAll(".dusk-hero__sun")) {
  const rays = sun.querySelector(".dusk-hero__rays");
  const disc = sun.querySelector("circle:last-of-type");
  const reduced = matchMedia("(prefers-reduced-motion: reduce)");
  let angle = 0;
  let target = 0;
  let settling = false;
  let last = 0;
  const settle = (now) => {
    angle = reduced.matches
      ? target
      : angle + (target - angle) * (1 - Math.exp((last - now) / 150));
    last = now;
    rays.style.transform = `rotate(${angle}deg)`;
    settling = Math.abs(target - angle) > 0.05;
    if (settling) {
      requestAnimationFrame(settle);
    }
  };
  window.addEventListener("pointermove", (event) => {
    const box = disc.getBoundingClientRect();
    const across = event.clientX - (box.left + box.width / 2);
    const above = box.top + box.height / 2 - event.clientY;
    if (Math.hypot(across, above) < box.width / 2) {
      return;
    }
    const aim = (Math.atan2(across, above) * 180) / Math.PI;
    let turn = (aim - target) % 360;
    if (turn > 180) {
      turn -= 360;
    } else if (turn < -180) {
      turn += 360;
    }
    target += turn;
    if (!settling) {
      settling = true;
      last = performance.now();
      requestAnimationFrame(settle);
    }
  });
}
