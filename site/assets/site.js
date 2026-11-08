// Tellfold site. Loaded in <head> so the page knows before first paint whether scripts and motion are on; the rest
// waits for the DOM. Without this file the reply slip shows every round stacked, with the answer, and nothing moves.
(function () {
  var root = document.documentElement;
  var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  root.classList.add('js');
  if (!still) root.classList.add('motion');

  var LAST = 2;            // rounds run 0..2 in the Mossbook example
  var STEP_MS = 3400;      // how long each round holds during the replay

  document.addEventListener('DOMContentLoaded', function () {
    var slip = document.querySelector('.slip');
    if (slip) setUpSlip(slip);
    revealSections();
  });

  function whenSeen(el, threshold, fn) {
    if (!('IntersectionObserver' in window)) { fn(); return; }
    var io = new IntersectionObserver(function (entries) {
      if (entries[0].isIntersecting) { io.disconnect(); fn(); }
    }, { threshold: threshold });
    io.observe(el);
  }

  function setUpSlip(slip) {
    var tabs = slip.querySelectorAll('.tab');
    var replay = slip.querySelector('.replay');
    var live = slip.querySelector('.rounds');
    var answer = slip.querySelector('.answer');
    var timers = [];

    function show(round) {
      slip.setAttribute('data-round', String(round));
      tabs.forEach(function (t) { t.setAttribute('aria-pressed', String(Number(t.dataset.go) === round)); });
      var panel = slip.querySelector('.round[data-r="' + round + '"]');
      if (panel && root.classList.contains('motion')) {
        // Drop the class and flush so the same round can print again.
        panel.classList.remove('printing');
        void panel.offsetWidth;
        panel.classList.add('printing');
      }
      if (round === LAST && !slip.hasAttribute('data-done')) {
        slip.setAttribute('data-done', '');
        if (answer && root.classList.contains('motion')) {
          answer.classList.remove('landing');
          void answer.offsetWidth;
          answer.classList.add('landing');
        }
      }
    }

    function stop() {
      timers.forEach(clearTimeout);
      timers = [];
      if (live) live.setAttribute('aria-live', 'polite');
    }

    function play() {
      stop();
      slip.removeAttribute('data-done');
      // The replay speaks for itself; announcing every automatic step would talk over the page.
      if (live) live.setAttribute('aria-live', 'off');
      for (var r = 0; r <= LAST; r++) timers.push(setTimeout(show.bind(null, r), STEP_MS * r));
      timers.push(setTimeout(function () { if (live) live.setAttribute('aria-live', 'polite'); }, STEP_MS * LAST + 50));
    }

    tabs.forEach(function (t) {
      t.addEventListener('click', function () { stop(); show(Number(t.dataset.go)); });
    });
    if (replay) replay.addEventListener('click', play);

    if (!root.classList.contains('motion')) {
      show(LAST);
      return;
    }

    // Wait on round 0 until the slip is actually on screen, then print the run.
    slip.setAttribute('data-round', '0');
    tabs.forEach(function (t) { t.setAttribute('aria-pressed', String(Number(t.dataset.go) === 0)); });
    whenSeen(slip, 0.25, play);
  }

  function revealSections() {
    var sections = document.querySelectorAll('main > section');
    if (!root.classList.contains('motion')) {
      sections.forEach(function (s) { s.classList.add('in'); });
      return;
    }
    sections.forEach(function (s) { whenSeen(s, 0.12, function () { s.classList.add('in'); }); });
  }
})();
