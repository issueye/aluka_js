class A { constructor(x){this.x=x;} get doubled(){return this.x*2;} static make(v){return new A(v);} m(){return 'A'+this.x;} }
class B extends A { constructor(x){super(x);} m(){return 'B'+super.m();} get doubled(){return super.doubled+1;} }
var b=new B(3);
console.log(JSON.stringify([b.x, b.doubled, b.m(), A.make(5).x, b instanceof A, b instanceof B]));
console.log(JSON.stringify([Object.getPrototypeOf(B)===A, Object.getPrototypeOf(b)===B.prototype]));
console.log(JSON.stringify([typeof A, A.name, A.length]));
var obj={v:1, get g(){return this.v*10;}, set g(n){this.v=n;}, method(){return 'm';}};
obj.g=4;
console.log(JSON.stringify([obj.g, obj.method(), obj.v]));
class C { static #p=1; static getP(){return C.#p;} }
console.log(JSON.stringify([C.getP()]));
