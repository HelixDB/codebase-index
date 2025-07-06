def respect_your_cat(food, num):
    # YOUR CODE GOES HERE #
    if len(food) < 5:
        return "Give me more food!"
    else:
        return food + "!" * num

def main():
    print(respect_your_cat("fish", 3))

if __name__ == "__main__":
    main()