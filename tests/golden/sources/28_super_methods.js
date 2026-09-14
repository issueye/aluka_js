class Parent {
    show() { return "parent"; }
}
class Child extends Parent {
    show() { return super.show() + "-child"; }
    setFlag(v) { super.flag = v; }
}
const c = new Child();
c.setFlag("on");
console.log(new Child().show());
console.log(c.flag);
