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

for (const fleet of document.querySelectorAll(".dusk-fleet__network")) {
  const arms = [...fleet.querySelectorAll(".dusk-arm")];
  const tinyLinks = [...fleet.querySelectorAll(".dusk-fleet__tiny path")];
  const tinyDots = [
    ...fleet.querySelectorAll(".dusk-fleet__tiny circle:not(.dusk-packet)"),
  ];
  const hub = fleet.querySelector(".dusk-fleet__hub");
  const glow = fleet.querySelector(".dusk-fleet__hub-glow");
  const hubLabel = fleet.querySelector(".dusk-fleet__hub-label");
  const layout = (width) => {
    const margin = 56;
    const scale = width < 600 ? 0.8 : 1;
    const ring = 36.77 * scale;
    const rx = width / 2 - margin;
    const spacing = scale < 1 ? 136 : 120;
    const around = (arms.length * spacing) / (2 * Math.PI);
    const ry = Math.max(
      200 * scale,
      Math.sqrt(Math.max(0, 2 * around * around - rx * rx)),
    );
    const height = 2 * ry + 2 * margin + 60 * scale;
    const cx = width / 2;
    const cy = margin + 10 + ry;
    const hubSize = Math.min(190, Math.max(150, width * 0.16));
    fleet.setAttribute("viewBox", `0 0 ${width} ${height}`);
    hub.setAttribute("x", cx - hubSize / 2);
    hub.setAttribute("y", cy - hubSize / 2);
    hub.setAttribute("width", hubSize);
    hub.setAttribute("height", hubSize);
    glow.setAttribute("cx", cx);
    glow.setAttribute("cy", cy);
    glow.setAttribute("r", hubSize * 0.62);
    hubLabel.setAttribute("x", cx);
    hubLabel.setAttribute("y", cy + hubSize / 2 + 26);
    const rim = [];
    let length = 0;
    let previous;
    for (let step = 0; step <= 720; step++) {
      const angle = -Math.PI / 2 + (2 * Math.PI * step) / 720;
      const point = [cx + rx * Math.cos(angle), cy + ry * Math.sin(angle)];
      if (previous) {
        length += Math.hypot(point[0] - previous[0], point[1] - previous[1]);
      }
      rim.push([length, point]);
      previous = point;
    }
    arms.forEach((arm, index) => {
      const along =
        (((index + 0.5) / arms.length) * length + Math.sin(index * 2.1) * 6) %
        length;
      const [, [px, py]] = rim.find(([at]) => at >= along);
      const out = 1 + 0.05 * Math.sin(index * 1.9);
      const x = cx + (px - cx) * out;
      const y = cy + (py - cy) * out;
      const dx = x - cx;
      const dy = y - cy;
      const reach = Math.hypot(dx, dy);
      const bend = (index % 2 ? -0.12 : 0.12) * reach;
      const qx = (cx + x) / 2 + (dy / reach) * bend;
      const qy = (cy + y) / 2 - (dx / reach) * bend;
      const toward = Math.hypot(qx - x, qy - y);
      const dotX = ((qx - x) / toward) * 36.77;
      const dotY = ((qy - y) / toward) * 36.77;
      arm
        .querySelector("path")
        .setAttribute("d", `M${cx} ${cy}Q${qx} ${qy} ${x} ${y}`);
      arm
        .querySelector(".dusk-node")
        .setAttribute("transform", `translate(${x} ${y}) scale(${scale})`);
      for (const light of arm.querySelectorAll(
        ".dusk-node__pulse, .dusk-node__online",
      )) {
        light.setAttribute("cx", dotX);
        light.setAttribute("cy", dotY);
      }
      const label = arm.querySelector(".dusk-node__label");
      label.setAttribute("x", x);
      label.setAttribute("y", y + ring + 21 * scale);
    });
    tinyDots.forEach((dot, index) => {
      const angle = index * 2.39996;
      const out = 0.3 + 0.8 * ((index * 0.618034) % 1);
      const x = cx + rx * 1.08 * out * Math.cos(angle);
      const y = cy + ry * 1.08 * out * Math.sin(angle);
      dot.setAttribute("cx", x);
      dot.setAttribute("cy", y);
      tinyLinks[index].setAttribute("d", `M${cx} ${cy}L${x} ${y}`);
    });
  };
  new ResizeObserver((entries) => {
    const width = entries[0].contentRect.width;
    if (width > 0) {
      layout(width);
    }
  }).observe(fleet.parentElement);
}
