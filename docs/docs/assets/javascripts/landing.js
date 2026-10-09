const still = matchMedia("(prefers-reduced-motion: reduce)");

for (const stack of document.querySelectorAll(".dusk-stack")) {
  const parts = [...stack.querySelectorAll("[data-layer]")];
  const head = stack.querySelector(".dusk-stack__head");
  const diagrams = [...stack.querySelectorAll(".dusk-stack__diagram")];
  const visible = new Set();
  let fronts = [];
  let current = "";
  const lift = (diagram, layer) => {
    const plate = diagram.querySelector(`.dusk-plate[data-layer="${layer}"]`);
    if (!plate) {
      return null;
    }
    const front = plate.cloneNode(true);
    front.classList.add("dusk-plate--front", "is-active");
    for (const aside of front.querySelectorAll(
      ".dusk-plate__leader, .dusk-plate__name",
    )) {
      aside.remove();
    }
    front.style.opacity = 0;
    diagram.append(front);
    for (const animation of front.getAnimations()) {
      const twin = plate
        .getAnimations()
        .find((original) => original.animationName === animation.animationName);
      if (animation.animationName && twin) {
        animation.currentTime = twin.currentTime;
      }
    }
    getComputedStyle(front).opacity;
    front.style.opacity = 1;
    return front;
  };
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          visible.add(entry.target);
        } else {
          visible.delete(entry.target);
        }
      }
      const reading = visible.values().next().value;
      const layer = reading ? reading.dataset.layer : "";
      if (layer === current) {
        return;
      }
      current = layer;
      for (const leaving of fronts) {
        leaving.style.opacity = 0;
        setTimeout(() => leaving.remove(), 450);
      }
      fronts = layer
        ? diagrams.map((diagram) => lift(diagram, layer)).filter(Boolean)
        : [];
      stack.classList.toggle("is-focused", Boolean(layer));
      for (const part of parts) {
        part.classList.toggle("is-active", part.dataset.layer === layer);
      }
      if (head && layer) {
        head.dataset.layer = layer;
      } else if (head) {
        delete head.dataset.layer;
      }
    },
    { rootMargin: "-49% 0px -50% 0px" },
  );
  for (const layer of stack.querySelectorAll(".dusk-layer")) {
    observer.observe(layer);
  }
}

const visibility = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    entry.target.classList.toggle("is-offscreen", !entry.isIntersecting);
    for (const drawing of entry.target.querySelectorAll(
      ".dusk-fleet__network, .dusk-stack__diagram",
    )) {
      if (entry.isIntersecting) {
        drawing.unpauseAnimations();
      } else {
        drawing.pauseAnimations();
      }
    }
  }
});
for (const section of document.querySelectorAll(
  ".dusk-hero, .dusk-fleet, .dusk-stack",
)) {
  visibility.observe(section);
}

if (matchMedia("(hover: hover)").matches) {
  for (const sun of document.querySelectorAll(".dusk-hero__sun")) {
    const rays = sun.querySelector(".dusk-hero__rays");
    const disc = sun.querySelector(".dusk-hero__disc");
    let aim = 0;
    window.addEventListener("pointermove", (event) => {
      if (event.pointerType === "touch" || still.matches) {
        return;
      }
      const box = disc.getBoundingClientRect();
      const across = event.clientX - (box.left + box.width / 2);
      const above = box.top + box.height / 2 - event.clientY;
      if (Math.hypot(across, above) < box.width / 2) {
        return;
      }
      let turn = ((Math.atan2(across, above) * 180) / Math.PI - aim) % 360;
      if (turn > 180) {
        turn -= 360;
      } else if (turn < -180) {
        turn += 360;
      }
      aim += turn;
      rays.style.setProperty("--dusk-aim", `${aim}deg`);
    });
  }
}
