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
  const nudge = () => {
    if (!settling) {
      settling = true;
      last = performance.now();
      requestAnimationFrame(settle);
    }
  };
  const turnTo = (aim) => {
    let turn = (aim - target) % 360;
    if (turn > 180) {
      turn -= 360;
    } else if (turn < -180) {
      turn += 360;
    }
    target += turn;
    nudge();
  };
  if (matchMedia("(hover: none)").matches) {
    window.addEventListener(
      "scroll",
      () => {
        target = window.scrollY * 0.2;
        nudge();
      },
      { passive: true },
    );
    continue;
  }
  window.addEventListener("pointermove", (event) => {
    if (event.pointerType === "touch") {
      return;
    }
    const box = disc.getBoundingClientRect();
    const across = event.clientX - (box.left + box.width / 2);
    const above = box.top + box.height / 2 - event.clientY;
    if (Math.hypot(across, above) < box.width / 2) {
      return;
    }
    turnTo((Math.atan2(across, above) * 180) / Math.PI);
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
    const narrow = matchMedia("(max-width: 37.4375em)").matches;
    const margin = narrow ? 50 : 56;
    const scale = narrow ? 0.8 : 1;
    const ring = 36.77 * scale;
    const rx = width / 2 - margin;
    const spacing = narrow ? 170 : 120;
    const around = (arms.length * spacing) / (2 * Math.PI);
    const hubSize = Math.min(190, Math.max(150, width * 0.16));
    const ry = Math.max(
      hubSize * 0.7 + 90 * scale,
      Math.sqrt(Math.max(0, 2 * around * around - rx * rx)),
    );
    const height = 2 * ry + 2 * margin + 60 * scale;
    const cx = width / 2;
    const cy = margin + 10 + ry;
    const hubHeight = hubSize * 1.41;
    hub.setAttribute("x", cx - hubSize / 2);
    hub.setAttribute("y", cy - hubHeight / 2);
    hub.setAttribute("width", hubSize);
    hub.setAttribute("height", hubHeight);
    glow.setAttribute("cx", cx);
    glow.setAttribute("cy", cy);
    glow.setAttribute("r", hubSize * 0.7);
    hubLabel.setAttribute("x", cx);
    hubLabel.setAttribute("y", cy + hubHeight / 2 + 24);
    const taken = [];
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
    const below = ring + 21 * scale;
    const spots = arms.map((arm, index) => {
      const along =
        (((index + 0.5) / arms.length) * length + Math.sin(index * 2.1) * 6) %
        length;
      const [, [px, py]] = rim.find(([at]) => at >= along);
      const out = 1 + 0.05 * Math.sin(index * 1.9);
      const fraction = (index + 0.5) / arms.length;
      return {
        x: cx + (px - cx) * out,
        y: cy + (py - cy) * out,
        half:
          arm.querySelector(".dusk-node__label").getComputedTextLength() / 2,
        end: Math.min(fraction, 1 - fraction, Math.abs(fraction - 0.5)) < 0.1,
      };
    });
    let top = 0;
    let bottom = height;
    if (narrow) {
      const clearance = 14;
      const outer = ring + 5 * scale;
      const boxes = (spot, y) => [
        [spot.x - outer, y - outer, spot.x + outer, y + outer],
        [spot.x - spot.half, y + below - 10, spot.x + spot.half, y + below + 3],
      ];
      const hubBand = [
        -Infinity,
        cy - hubHeight / 2,
        Infinity,
        cy + hubHeight / 2 + 40,
      ];
      const overlaps = (one, other) =>
        one[0] < other[2] + clearance &&
        other[0] < one[2] + clearance &&
        one[1] < other[3] + clearance &&
        other[1] < one[3] + clearance;
      for (const spot of spots.filter((spot) => spot.end)) {
        const obstacles = [
          hubBand,
          ...spots
            .filter((other) => other !== spot)
            .flatMap((other) => boxes(other, other.y)),
        ];
        const toward = Math.sign(cy - spot.y);
        while (
          !boxes(spot, spot.y + toward * 2).some((own) =>
            obstacles.some((obstacle) => overlaps(own, obstacle)),
          )
        ) {
          spot.y += toward * 2;
        }
      }
      spots.forEach((spot, index) => {
        const mirror = spots[spots.length - 1 - index];
        if (spot.end) {
          const reach = Math.max(
            Math.abs(spot.y - cy),
            Math.abs(mirror.y - cy),
          );
          spot.y = cy + Math.sign(spot.y - cy) * reach;
        }
      });
      top = Math.min(...spots.map((spot) => spot.y - ring)) - 24;
      bottom = Math.max(...spots.map((spot) => spot.y + below + 3)) + 24;
    }
    fleet.setAttribute("viewBox", `0 ${top} ${width} ${bottom - top}`);
    arms.forEach((arm, index) => {
      const { x, y } = spots[index];
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
      label.setAttribute("y", y + below);
      taken.push([x, y, 70 * scale]);
    });
    const placed = [];
    for (const [index, arm] of arms.entries()) {
      const label = arm.querySelector(".dusk-node__label");
      const half = label.getComputedTextLength() / 2;
      const x = Number(label.getAttribute("x"));
      let y = Number(label.getAttribute("y"));
      const clashes = (top) =>
        placed.some(
          (other) =>
            Math.abs(other.y - top) < 18 * scale &&
            x - half < other.right + 18 * scale &&
            other.left < x + half + 18 * scale,
        );
      if (clashes(y)) {
        y = taken[index][1] - ring - 10 * scale;
        label.setAttribute("y", y);
      }
      placed.push({ left: x - half, right: x + half, y });
    }
    let seed = 7;
    const random = () => {
      seed = (seed * 48271) % 2147483647;
      return seed / 2147483647;
    };
    tinyDots.forEach((dot, index) => {
      let x = cx;
      let y = cy;
      for (let attempt = 0; attempt < 60; attempt++) {
        const angle = random() * 2 * Math.PI;
        const out = 0.2 + 0.9 * Math.sqrt(random());
        x = cx + rx * 1.1 * out * Math.cos(angle);
        y = cy + ry * 1.1 * out * Math.sin(angle);
        if (
          y > top + 8 &&
          y < bottom - 8 &&
          Math.hypot(x - cx, y - cy) > hubSize * 0.75 &&
          taken.every(([tx, ty, room]) => Math.hypot(tx - x, ty - y) > room)
        ) {
          break;
        }
      }
      taken.push([x, y, 44]);
      dot.setAttribute("cx", x);
      dot.setAttribute("cy", y);
      tinyLinks[index].setAttribute("d", `M${cx} ${cy}L${x} ${y}`);
    });
  };
  let current = 0;
  new ResizeObserver((entries) => {
    const width = entries[0].contentRect.width;
    if (width > 0) {
      current = width;
      layout(width);
    }
  }).observe(fleet.parentElement);
  document.fonts.ready.then(() => {
    if (current > 0) {
      layout(current);
    }
  });
}

const still = matchMedia("(prefers-reduced-motion: reduce)");
const tracked = [
  ...[...document.querySelectorAll(".dusk-hero__sun")].map((element) => ({
    element,
    name: "--dusk-sunk",
    aim: () => Math.min(1, window.scrollY / (window.innerHeight * 0.6)),
  })),
  ...[...document.querySelectorAll(".dusk-section > h2")].map((element) => ({
    element,
    name: "--dusk-passed",
    aim: () => {
      const box = element.getBoundingClientRect();
      const passed =
        (window.innerHeight - box.top) / (window.innerHeight + box.height);
      return Math.max(0, Math.min(1, passed));
    },
  })),
  ...[...document.querySelectorAll(".dusk-layer")].map((element) => ({
    element,
    name: "--dusk-read",
    aim: () => {
      const box = element.getBoundingClientRect();
      const read = (window.innerHeight / 2 - box.top) / box.height;
      return Math.max(0, Math.min(1, read));
    },
  })),
].map((item) => ({ ...item, target: item.aim(), value: item.aim() }));
let gliding = false;
let lastGlide = 0;
const glide = (now) => {
  const ease = 1 - Math.exp((lastGlide - now) / 120);
  lastGlide = now;
  gliding = false;
  for (const item of tracked) {
    item.value += (item.target - item.value) * ease;
    if (Math.abs(item.target - item.value) > 0.0005) {
      gliding = true;
    } else {
      item.value = item.target;
    }
    item.element.style.setProperty(item.name, item.value);
  }
  if (gliding) {
    requestAnimationFrame(glide);
  }
};
const follow = () => {
  if (still.matches) {
    return;
  }
  for (const item of tracked) {
    item.target = item.aim();
  }
  if (!gliding) {
    gliding = true;
    lastGlide = performance.now();
    requestAnimationFrame(glide);
  }
};
window.addEventListener("scroll", follow, { passive: true });
window.addEventListener("resize", follow);
for (const item of tracked) {
  item.element.style.setProperty(item.name, item.value);
}
