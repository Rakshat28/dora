interface IConfig {
    host: string;
    port: number;
}

interface ILogger {
    log(message: string): void;
}

interface IEvent {
    name: string;
}

type RegularType = {
    value: string;
};

class Config implements IConfig {
    host = "localhost";
    port = 3000;
}
