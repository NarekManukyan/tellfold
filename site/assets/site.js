// Tellfold site. Loaded in <head> so the page knows before first paint whether scripts and motion are on; the rest
// waits for the DOM. Without this file the page shows the finished run (round 2 and its verdict) and nothing moves.
(function () {
  var root = document.documentElement;
  var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  root.classList.add('js');
  if (!still) root.classList.add('motion');

  var ROUNDS = 3;
  var STEP_MS = 3200;      // how long each round holds during the replay
  var FAN_MS = 1500;       // the fan and its slips are out by then
  var FOLD_MS = 460;       // folding shut before a replay

  document.addEventListener('DOMContentLoaded', function () {
    var hero = document.querySelector('.hero');
    if (hero) setUpHero(hero);
    revealSections();
  });

  function whenSeen(el, threshold, fn) {
    if (!('IntersectionObserver' in window)) { fn(); return; }
    var io = new IntersectionObserver(function (entries) {
      if (entries[0].isIntersecting) { io.disconnect(); fn(); }
    }, { threshold: threshold });
    io.observe(el);
  }

  function setUpHero(hero) {
    var buttons = hero.querySelectorAll('.scrub-btn');
    var play = hero.querySelector('.scrub-play');
    var readout = hero.querySelector('.readout');
    var stageLabel = hero.querySelector('.js-stage');
    var STAGES = ['The announcement goes out', 'Round 1 of 2: the crowd reacts', 'Round 2 of 2'];
    var timers = [];

    hero.style.setProperty('--step', STEP_MS + 'ms');

    var postBoxes = hero.querySelectorAll('.posts');

    // Each slip grows or shrinks to the round's post instead of jumping: measure the incoming post, ease the box.
    function fitPosts(round) {
      postBoxes.forEach(function (box) {
        var post = box.querySelector('.post[data-r="' + round + '"]');
        if (post) box.style.height = post.offsetHeight + 'px';
      });
    }

    function show(round) {
      fitPosts(round);
      hero.setAttribute('data-round', String(round));
      buttons.forEach(function (b) { b.setAttribute('aria-pressed', String(Number(b.dataset.go) === round)); });
      if (stageLabel) stageLabel.textContent = STAGES[round];
    }

    function stop() {
      timers.forEach(clearTimeout);
      timers = [];
      hero.classList.remove('playing');
      if (readout) readout.setAttribute('aria-live', 'polite');
    }

    // Show a round and restart the timer on its button: drop the class, flush, add it back.
    function tick(round) {
      show(round);
      hero.classList.remove('playing');
      if (round < ROUNDS - 1) { void hero.offsetWidth; hero.classList.add('playing'); }
      else if (readout) readout.setAttribute('aria-live', 'polite');
    }

    function autoplay(delay) {
      // The replay speaks for itself; announcing every automatic step would talk over the page.
      if (readout) readout.setAttribute('aria-live', 'off');
      for (var r = 0; r < ROUNDS; r++) timers.push(setTimeout(tick.bind(null, r), delay + STEP_MS * r));
    }

    function open() {
      // Two frames, so the folded state is painted before the transition to open starts.
      requestAnimationFrame(function () {
        requestAnimationFrame(function () { hero.classList.add('open'); });
      });
      autoplay(FAN_MS);
    }

    // Text reflows when the web font arrives or the window resizes; refit the current round without easing.
    function refit() {
      hero.classList.remove('fitted');
      fitPosts(Number(hero.getAttribute('data-round')));
      requestAnimationFrame(function () { hero.classList.add('fitted'); });
    }
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(refit);
    var resizeTimer;
    window.addEventListener('resize', function () { clearTimeout(resizeTimer); resizeTimer = setTimeout(refit, 120); });

    buttons.forEach(function (b) {
      b.addEventListener('click', function () { stop(); hero.classList.add('open'); show(Number(b.dataset.go)); });
    });

    if (!root.classList.contains('motion')) {
      hero.classList.add('open');
      show(ROUNDS - 1);
      if (play) play.addEventListener('click', function () { stop(); autoplay(0); });
      return;
    }

    // Folded, waiting on round 0, until the hero is actually on screen.
    show(0);
    requestAnimationFrame(function () { hero.classList.add('fitted'); });
    whenSeen(hero, 0.3, open);

    if (play) play.addEventListener('click', function () {
      stop();
      hero.classList.remove('open');
      show(0);
      timers.push(setTimeout(open, FOLD_MS));
    });
  }

  function revealSections() {
    var sections = document.querySelectorAll('main > section:not(.hero)');
    if (!root.classList.contains('motion')) {
      sections.forEach(function (s) { s.classList.add('in'); });
      return;
    }
    sections.forEach(function (s) { whenSeen(s, 0.08, function () { s.classList.add('in'); }); });
  }
})();
