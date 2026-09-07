class Parent {
    show() { return "parent"; }
}
class Child extends Parent {
    show() { return super.show() + "-child"; }
}
console.log(new Child().show());
