(function(){
  var root=document.documentElement;
  function setLang(l){
    root.lang=l;
    document.querySelectorAll('.lang button').forEach(function(b){b.classList.toggle('on',b.dataset.set===l)});
    try{localStorage.setItem('lang',l)}catch(e){}
  }
  var saved=null;try{saved=localStorage.getItem('lang')}catch(e){}
  setLang(saved||((navigator.language||'en').toLowerCase().indexOf('de')===0?'de':'en'));
  document.querySelectorAll('.lang button').forEach(function(b){b.addEventListener('click',function(){setLang(b.dataset.set)})});
})();
