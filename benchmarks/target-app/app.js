const ui = Object.fromEntries(['projects','board','status','search','undo','new-card','editor','card-id','card-title','card-description','card-priority','dialog-title','dialog-error','save-card','toast'].map(id => [id.replace(/-([a-z])/g,(_,c)=>c.toUpperCase()), document.getElementById(id)]));
let state; let projectId; let draggedId; let toastTimer;

async function request(path, options = {}) {
  const response = await fetch(path, { headers: { 'content-type': 'application/json' }, ...options });
  const payload = await response.json();
  if (!response.ok) throw new Error(payload.error || `Request failed (${response.status})`);
  return payload;
}

async function load() {
  ui.status.textContent = 'Loading board…';
  try { state = await request('/api/state'); projectId ||= state.projects[0].id; render(); ui.status.textContent = `Revision ${state.revision}`; }
  catch (error) { ui.status.textContent = `Could not load board: ${error.message}`; }
}

function render() {
  ui.projects.replaceChildren(...state.projects.map(project => {
    const button = document.createElement('button'); button.textContent = project.name; button.setAttribute('aria-selected', project.id === projectId); button.onclick = () => { projectId = project.id; render(); }; return button;
  }));
  const query = ui.search.value.trim().toLowerCase();
  ui.board.replaceChildren(...state.columns.map(column => {
    const section = document.createElement('section'); section.className = 'column';
    const cards = state.cards.filter(card => card.projectId === projectId && card.columnId === column.id && `${card.title} ${card.description}`.toLowerCase().includes(query));
    section.innerHTML = `<h2>${column.name}<span class="count">${cards.length}</span></h2><div class="card-list" data-column="${column.id}"></div>`;
    const list = section.querySelector('.card-list');
    for (const card of cards) list.append(cardElement(card));
    list.ondragover = event => { event.preventDefault(); list.classList.add('over'); };
    list.ondragleave = () => list.classList.remove('over');
    list.ondrop = event => { event.preventDefault(); list.classList.remove('over'); moveCard(draggedId, column.id); };
    return section;
  }));
}

function cardElement(card) {
  const article = document.createElement('article'); article.className = `card${card.pending ? ' pending' : ''}`; article.draggable = true; article.tabIndex = 0; article.dataset.id = card.id;
  article.innerHTML = `<h3></h3><p></p><span class="priority"></span>`; article.querySelector('h3').textContent = card.title; article.querySelector('p').textContent = card.description; article.querySelector('.priority').textContent = card.priority;
  article.ondragstart = () => { draggedId = card.id; }; article.ondblclick = () => openEditor(card); article.onkeydown = event => { if (event.key === 'Enter') openEditor(card); }; return article;
}

async function moveCard(id, columnId) {
  const card = state.cards.find(candidate => candidate.id === id); if (!card || card.columnId === columnId) return;
  const previous = card.columnId; card.columnId = columnId; card.pending = true; render();
  try { const result = await request(`/api/cards/${id}`, { method:'PATCH', body:JSON.stringify({ columnId }) }); Object.assign(card, result.card, { pending:false }); state.revision = result.revision; notice('Card moved'); }
  catch (error) { card.columnId = previous; card.pending = false; notice(`Move failed: ${error.message}`); }
  render();
}

function openEditor(card = null) { ui.cardId.value = card?.id || ''; ui.cardTitle.value = card?.title || ''; ui.cardDescription.value = card?.description || ''; ui.cardPriority.value = card?.priority || 'medium'; ui.dialogTitle.textContent = card ? 'Edit card' : 'New card'; ui.dialogError.textContent = ''; ui.editor.showModal(); queueMicrotask(() => ui.cardTitle.focus()); }

ui.editor.addEventListener('close', async () => {
  if (ui.editor.returnValue !== 'default') return;
  const input = { title:ui.cardTitle.value, description:ui.cardDescription.value, priority:ui.cardPriority.value, projectId };
  ui.dialogError.textContent = ''; ui.saveCard.disabled = true;
  try { const id = ui.cardId.value; const result = await request(id ? `/api/cards/${id}` : '/api/cards', { method:id?'PATCH':'POST', body:JSON.stringify(input) }); if (id) Object.assign(state.cards.find(card=>card.id===id), result.card); else state.cards.push(result.card); state.revision=result.revision; render(); notice(id?'Card saved':'Card created'); }
  catch (error) { ui.dialogError.textContent = error.message; ui.editor.showModal(); }
  finally { ui.saveCard.disabled = false; }
});

ui.search.oninput = () => render(); ui.newCard.onclick = () => openEditor(); ui.undo.onclick = async () => { try { state = await request('/api/undo',{method:'POST',body:'{}'}); render(); notice('Last change undone'); } catch(error) { notice(error.message); } };
document.addEventListener('keydown', event => { if ((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='k') { event.preventDefault(); ui.search.focus(); } if ((event.ctrlKey||event.metaKey) && event.key.toLowerCase()==='z' && !ui.editor.open) { event.preventDefault(); ui.undo.click(); } if (event.key==='n' && !ui.editor.open && document.activeElement===document.body) openEditor(); });
function notice(message) { clearTimeout(toastTimer); ui.toast.textContent=message; ui.toast.classList.add('show'); toastTimer=setTimeout(()=>ui.toast.classList.remove('show'),1800); }
load();
