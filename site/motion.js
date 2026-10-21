/* Visible by default. Only below-viewport leaf groups are prepared for entrance. */
function installReferenceReveals(root = document) {
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const animations = new Map();
  const elements = [...root.querySelectorAll('[data-reveal]')];
  const finish = element => {
    element.dataset.revealPlayed = 'true';
    animations.get(element)?.cancel();
    animations.delete(element);
  };
  const observer = typeof IntersectionObserver === 'function'
    ? new IntersectionObserver(entries => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const element = entry.target;
        observer.unobserve(element);
        element.dataset.revealPlayed = 'true';
        if (reduced.matches || document.visibilityState === 'hidden' || entry.boundingClientRect.bottom <= 0) finish(element);
        else animations.get(element)?.play();
      }
    }, { threshold: 0, rootMargin: '0px 0px 80px 0px' }) : undefined;
  const stop = () => { observer?.disconnect(); elements.forEach(finish); };
  const onPreference = () => { if (reduced.matches) stop(); };
  reduced.addEventListener('change', onPreference);
  for (const element of elements) {
    if (element.dataset.revealPlayed === 'true') continue;
    const rect = element.getBoundingClientRect();
    if (reduced.matches || !observer || !element.animate || rect.top < innerHeight || document.visibilityState === 'hidden') {
      finish(element);
      continue;
    }
    const transform = element.dataset.reveal === 'slide' ? 'translateX(16px)'
      : element.dataset.reveal === 'fade' ? 'none' : 'translateY(12px)';
    const animation = element.animate([{ opacity: 0, transform }, { opacity: 1, transform: 'none' }], {
      duration: Number(element.dataset.duration || 520),
      delay: Math.min(240, Number(element.dataset.delay || 0)),
      easing: 'cubic-bezier(0.22, 1, 0.36, 1)', fill: 'backwards'
    });
    animation.pause();
    animation.onfinish = () => finish(element);
    animations.set(element, animation);
    observer.observe(element);
  }
  let lastY = scrollY;
  let pending = false;
  const onScroll = () => {
    const rapid = Math.abs(scrollY - lastY) > 100;
    lastY = scrollY;
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      for (const [element, animation] of animations) {
        const rect = element.getBoundingClientRect();
        if (rect.bottom <= 0 || (rapid && (rect.top < innerHeight + 80 || animation.playState === 'running'))) {
          observer?.unobserve(element);
          finish(element);
        }
      }
    });
  };
  const onResize = () => {
    for (const element of animations.keys()) {
      if (element.getBoundingClientRect().top < innerHeight) {
        observer?.unobserve(element);
        finish(element);
      }
    }
  };
  addEventListener('scroll', onScroll, { passive: true });
  addEventListener('resize', onResize);
  return () => {
    stop();
    reduced.removeEventListener('change', onPreference);
    removeEventListener('scroll', onScroll);
    removeEventListener('resize', onResize);
  };
}

function playHeroEntrance() {
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const navigation = performance.getEntriesByType('navigation')[0];
  if (reduced.matches || scrollY > 0 || location.hash || navigation?.type === 'back_forward') return () => {};
  const owned = [];
  for (const [selector, distance, duration, delay] of [
    ['[data-hero-copy]', 20, 680, 0], ['[data-hero-media]', 12, 720, 80]
  ]) {
    const element = document.querySelector(selector);
    if (!element?.animate) continue;
    const animation = element.animate([{ transform: `translateY(${distance}px)` }, { transform: 'none' }], {
      duration, delay, easing: 'cubic-bezier(0.22, 1, 0.36, 1)', fill: 'backwards'
    });
    animation.onfinish = () => animation.cancel();
    owned.push(animation);
  }
  const stop = () => owned.forEach(animation => animation.cancel());
  const onPreference = () => { if (reduced.matches) stop(); };
  reduced.addEventListener('change', onPreference);
  return () => { stop(); reduced.removeEventListener('change', onPreference); };
}

async function verifyFonts() {
  await document.fonts.ready;
  const required = [['Figtree', '400'], ['Figtree', '500'], ['Figtree', '600'], ['Figtree', '700'], ['Cormorant Garamond', '500'], ['Cormorant Garamond', '600']];
  document.documentElement.dataset.fonts = required.every(([family, weight]) => {
    const face = [...document.fonts].find(font => font.family.replace(/['"]/g, '') === family && font.status === 'loaded');
    return Boolean(face) && document.fonts.check(`${weight} 16px "${family}"`);
  }) ? 'loaded' : 'fallback';
}

function bindNavigation() {
  const disclosure = document.querySelector('.mobile-nav');
  const summary = disclosure.querySelector('summary');
  summary.setAttribute('aria-expanded', String(disclosure.open));
  disclosure.addEventListener('toggle', () => summary.setAttribute('aria-expanded', String(disclosure.open)));
  disclosure.addEventListener('keydown', event => {
    if (event.key === 'Escape' && disclosure.open) {
      disclosure.open = false;
      summary.setAttribute('aria-expanded', 'false');
      summary.focus();
    }
  });
  document.querySelectorAll('a[href^="#"]').forEach(link => {
    link.addEventListener('click', () => {
      const target = document.getElementById(link.hash.slice(1));
      if (!target) return;
      disclosure.open = false;
      summary.setAttribute('aria-expanded', 'false');
      // Preserve native anchors and browser history; move keyboard focus without another scroll.
      requestAnimationFrame(() => target.focus({ preventScroll: true }));
    });
  });
}

bindNavigation();
verifyFonts();
const cleanupHero = playHeroEntrance();
const cleanupReveals = installReferenceReveals();
addEventListener('pagehide', () => { cleanupHero(); cleanupReveals(); }, { once: true });
