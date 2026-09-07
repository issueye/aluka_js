class Base {
    constructor(name) {
        this.name = name;
    }
    greet() {
        return "Hello " + this.name;
    }
}
class Derived extends Base {
    constructor(name, title) {
        super(name);
        this.title = title;
    }
    greet() {
        return super.greet() + " (" + this.title + ")";
    }
}
const d = new Derived("Alice", "Engineer");
console.log(d.greet(), d instanceof Base, d instanceof Derived);
