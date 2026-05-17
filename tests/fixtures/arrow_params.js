const simple = (a) => a + 1;

const multiParam = (x, y, z) => {
    return x + y + z;
};

const withDefaults = (a = 10, b = 20) => a * b;

const destructured = ({ name, age }) => {
    return `${name} is ${age}`;
};
