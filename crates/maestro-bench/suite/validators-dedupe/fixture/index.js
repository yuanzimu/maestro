function is_email(v) {
  const s = String(v).trim();
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(s);
}

function is_phone(v) {
  const s = String(v).trim();
  return /^\+?\d{7,15}$/.test(s);
}

module.exports = { is_email, is_phone };
