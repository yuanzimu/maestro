const RULES = {
  email: /^[^\s@]+@[^\s@]+\.[^\s@]+$/,
  phone: /^\+?\d{7,15}$/,
};

function validate(value, kind) {
  const rule = RULES[kind];
  if (!rule) {
    throw new Error(`未知校验类型: ${kind}`);
  }
  return rule.test(String(value).trim());
}

function is_email(v) {
  return validate(v, "email");
}

function is_phone(v) {
  return validate(v, "phone");
}

module.exports = { is_email, is_phone, validate };
