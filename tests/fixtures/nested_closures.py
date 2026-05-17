def outer_function(x):
    def middle_function(y):
        def inner_closure(z):
            return x + y + z
        return inner_closure
    return middle_function

def another_function():
    pass
