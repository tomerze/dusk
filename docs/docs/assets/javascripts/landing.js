const still = matchMedia("(prefers-reduced-motion: reduce)");

document.querySelectorAll(".dusk-stack").forEach((stack) => {
  const parts = stack.querySelectorAll("[data-layer]");
  const diagram = stack.querySelector(".dusk-stack__diagram");
  const plates = [...diagram.querySelectorAll(".dusk-plate")];
  const visible = new Set();
  let front = null;
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
      if (front && front.dataset.layer !== layer) {
        const leaving = front;
        leaving.classList.remove("is-active");
        leaving.style.opacity = 0;
        setTimeout(() => leaving.remove(), 450);
        front = null;
      }
      const plate = plates.find(
        (candidate) => candidate.dataset.layer === layer,
      );
      if (plate && !front) {
        front = plate.cloneNode(true);
        front.classList.add("dusk-plate--front");
        front.classList.remove("is-active");
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
            .find(
              (original) => original.animationName === animation.animationName,
            );
          if (animation.animationName && twin) {
            animation.currentTime = twin.currentTime;
          }
        }
        getComputedStyle(front).opacity;
        front.style.opacity = 1;
      }
      stack.classList.toggle("is-focused", Boolean(current));
      for (const part of parts) {
        part.classList.toggle("is-active", part.dataset.layer === layer);
      }
      if (front) {
        front.classList.add("is-active");
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
      ".dusk-fleet__network, .dusk-stack__diagram",
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

for (const head of document.querySelectorAll(".dusk-stack__head")) {
  const layers = [...head.parentElement.querySelectorAll(".dusk-layer")];
  let reading = null;
  const read = () => {
    const at = head.getBoundingClientRect().top;
    const start = layers[0].getBoundingClientRect().top;
    const end = layers.at(-1).getBoundingClientRect().bottom;
    const layer =
      at > start + 1 && at < end - 1
        ? layers.find(
            (candidate) => candidate.getBoundingClientRect().bottom > at,
          )
        : null;
    if (layer === reading) {
      return;
    }
    reading = layer;
    head.classList.toggle("is-reading", Boolean(layer));
    if (layer) {
      head.style.setProperty(
        "--dusk-ink",
        getComputedStyle(layer).getPropertyValue("--dusk-ink"),
      );
    }
  };
  window.addEventListener("scroll", read, { passive: true });
  window.addEventListener("resize", read);
  read();
}
